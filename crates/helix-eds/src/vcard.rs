//! vCard (RFC 6350 / 2426, with 2.1 tolerance) parsing and serialization, plus
//! a typed [`Contact`] view that preserves properties it does not manage.

use crate::contentline::{self, escape_text, fold, Property};
use crate::nc_;
use base64::Engine;
use std::path::{Path, PathBuf};

/// A raw vCard: an ordered list of properties (without BEGIN/END).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VCard {
    pub props: Vec<Property>,
}

impl VCard {
    pub fn get(&self, name: &str) -> Option<&Property> {
        self.props
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
    }

    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Property> + 'a {
        self.props
            .iter()
            .filter(move |p| p.name.eq_ignore_ascii_case(name))
    }

    pub fn remove_all(&mut self, name: &str) {
        self.props.retain(|p| !p.name.eq_ignore_ascii_case(name));
    }

    pub fn version(&self) -> &str {
        self.get("VERSION")
            .map(|p| p.value.as_str())
            .unwrap_or("3.0")
    }

    /// Serializes with CRLF line endings and folding.
    pub fn to_string_crlf(&self) -> String {
        let mut out = String::from("BEGIN:VCARD\r\n");
        if self.get("VERSION").is_none() {
            out.push_str("VERSION:3.0\r\n");
        }
        for p in &self.props {
            out.push_str(&fold(&p.to_line()));
            out.push_str("\r\n");
        }
        out.push_str("END:VCARD\r\n");
        out
    }
}

/// Parses every vCard in `text` (a `.vcf` file may hold many).
pub fn parse_many(text: &str) -> Vec<VCard> {
    let mut out = Vec::new();
    let mut cur: Option<VCard> = None;
    let mut depth = 0;
    for line in contentline::unfold(text) {
        let Some(p) = contentline::parse_line(&line) else {
            continue;
        };
        if p.name == "BEGIN" && p.value.eq_ignore_ascii_case("VCARD") {
            depth += 1;
            if depth == 1 {
                cur = Some(VCard::default());
            }
            continue;
        }
        if p.name == "END" && p.value.eq_ignore_ascii_case("VCARD") {
            depth -= 1;
            if depth == 0 {
                if let Some(c) = cur.take() {
                    out.push(c);
                }
            }
            continue;
        }
        if depth == 1 {
            if let Some(c) = cur.as_mut() {
                c.props.push(p);
            }
        }
    }
    out
}

pub fn parse(text: &str) -> Option<VCard> {
    parse_many(text).into_iter().next()
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StructuredName {
    pub family: String,
    pub given: String,
    pub additional: String,
    pub prefix: String,
    pub suffix: String,
}

impl StructuredName {
    pub fn is_empty(&self) -> bool {
        [
            &self.family,
            &self.given,
            &self.additional,
            &self.prefix,
            &self.suffix,
        ]
        .iter()
        .all(|s| s.is_empty())
    }

    /// "Prefix Given Additional Family Suffix" with empty parts skipped.
    pub fn display(&self) -> String {
        [
            &self.prefix,
            &self.given,
            &self.additional,
            &self.family,
            &self.suffix,
        ]
        .iter()
        .filter(|s| !s.is_empty())
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(" ")
    }

    /// Splits a free-form full name into given/family parts (last word is the
    /// family name).
    pub fn guess_from(full: &str) -> StructuredName {
        let words: Vec<&str> = full.split_whitespace().collect();
        match words.len() {
            0 => StructuredName::default(),
            1 => StructuredName {
                given: words[0].to_string(),
                ..Default::default()
            },
            n => StructuredName {
                given: words[0].to_string(),
                additional: words[1..n - 1].join(" "),
                family: words[n - 1].to_string(),
                ..Default::default()
            },
        }
    }
}

/// A typed value such as an email address or phone number.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Typed {
    /// Upper-cased TYPE values (e.g. `HOME`, `CELL`), without `PREF`.
    pub types: Vec<String>,
    pub value: String,
    pub pref: bool,
}

impl Typed {
    pub fn new(types: &[&str], value: &str) -> Typed {
        Typed {
            types: types.iter().map(|t| t.to_ascii_uppercase()).collect(),
            value: value.to_string(),
            pref: false,
        }
    }

    pub fn has(&self, t: &str) -> bool {
        self.types.iter().any(|x| x.eq_ignore_ascii_case(t))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Address {
    pub types: Vec<String>,
    pub po_box: String,
    pub extended: String,
    pub street: String,
    pub locality: String,
    pub region: String,
    pub postal_code: String,
    pub country: String,
}

impl Address {
    pub fn is_empty(&self) -> bool {
        [
            &self.po_box,
            &self.extended,
            &self.street,
            &self.locality,
            &self.region,
            &self.postal_code,
            &self.country,
        ]
        .iter()
        .all(|s| s.trim().is_empty())
    }

    /// Multi-line postal formatting (street, extended, PO box, "locality
    /// region postal code", country).
    pub fn formatted(&self) -> String {
        let mut lines: Vec<String> = Vec::new();
        for l in [&self.street, &self.extended, &self.po_box] {
            if !l.trim().is_empty() {
                lines.push(l.trim().to_string());
            }
        }
        let city: Vec<&str> = [&self.locality, &self.region, &self.postal_code]
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if !city.is_empty() {
            lines.push(city.join(" "));
        }
        if !self.country.trim().is_empty() {
            lines.push(self.country.trim().to_string());
        }
        lines.join("\n")
    }

    /// One-line form for map searches.
    pub fn one_line(&self) -> String {
        self.formatted().replace('\n', ", ")
    }
}

/// Contact photo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Photo {
    /// Inline image bytes with an optional MIME type.
    Inline { mime: Option<String>, data: Vec<u8> },
    /// External URI (often `file://` for EDS-managed photos).
    Uri(String),
}

/// A contact. Properties not represented here are kept in `extra` and
/// written back unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Contact {
    pub uid: String,
    pub full_name: String,
    pub name: StructuredName,
    pub nickname: String,
    pub org: String,
    pub org_unit: String,
    pub title: String,
    pub role: String,
    pub emails: Vec<Typed>,
    pub phones: Vec<Typed>,
    pub addresses: Vec<Address>,
    pub urls: Vec<Typed>,
    pub ims: Vec<Typed>,
    /// Raw BDAY value (`1990-05-17`, `19900517`, `--0517`).
    pub birthday: String,
    pub anniversary: String,
    pub note: String,
    pub categories: Vec<String>,
    pub photo: Option<Photo>,
    /// `X-EVOLUTION-FILE-AS` (EDS requires it).
    pub file_as: String,
    pub is_list: bool,
    pub rev: String,
    /// Unmanaged properties, in original order.
    pub extra: Vec<Property>,
    pub version: String,
}

const MANAGED: &[&str] = &[
    "VERSION",
    "UID",
    "FN",
    "N",
    "NICKNAME",
    "ORG",
    "TITLE",
    "ROLE",
    "EMAIL",
    "TEL",
    "ADR",
    "URL",
    "IMPP",
    "BDAY",
    "ANNIVERSARY",
    "X-EVOLUTION-ANNIVERSARY",
    "NOTE",
    "CATEGORIES",
    "PHOTO",
    "X-EVOLUTION-FILE-AS",
    "X-EVOLUTION-LIST",
    "REV",
];

fn types_of(p: &Property) -> (Vec<String>, bool) {
    let mut pref = p.param("PREF").is_some();
    let mut types = Vec::new();
    for t in p.param_values("TYPE") {
        let t = t.to_ascii_uppercase();
        if t == "PREF" {
            pref = true;
        } else if !types.contains(&t) {
            types.push(t);
        }
    }
    (types, pref)
}

fn typed(p: &Property) -> Typed {
    let (types, pref) = types_of(p);
    Typed {
        types,
        value: p.text_value(),
        pref,
    }
}

impl Contact {
    pub fn from_vcard(card: &VCard) -> Contact {
        let mut c = Contact {
            version: card.version().to_string(),
            ..Default::default()
        };
        for p in &card.props {
            match p.name.as_str() {
                "VERSION" => {}
                "UID" => c.uid = p.text_value(),
                "FN" => c.full_name = p.text_value(),
                "N" => {
                    let s = p.structured();
                    let get = |i: usize| s.get(i).cloned().unwrap_or_default();
                    c.name = StructuredName {
                        family: get(0),
                        given: get(1),
                        additional: get(2),
                        prefix: get(3),
                        suffix: get(4),
                    };
                }
                "NICKNAME" => c.nickname = p.list().join(", "),
                "ORG" => {
                    let s = p.structured();
                    c.org = s.first().cloned().unwrap_or_default();
                    c.org_unit = s.get(1..).map(|r| r.join(", ")).unwrap_or_default();
                }
                "TITLE" => c.title = p.text_value(),
                "ROLE" => c.role = p.text_value(),
                "EMAIL" => c.emails.push(typed(p)),
                "TEL" => {
                    let mut t = typed(p);
                    t.value = t.value.trim_start_matches("tel:").to_string();
                    c.phones.push(t);
                }
                "URL" => c.urls.push(typed(p)),
                "IMPP" => c.ims.push(typed(p)),
                "ADR" => {
                    let s = p.structured();
                    let get = |i: usize| s.get(i).cloned().unwrap_or_default();
                    let (types, _) = types_of(p);
                    c.addresses.push(Address {
                        types,
                        po_box: get(0),
                        extended: get(1),
                        street: get(2),
                        locality: get(3),
                        region: get(4),
                        postal_code: get(5),
                        country: get(6),
                    });
                }
                "BDAY" => c.birthday = p.text_value(),
                "ANNIVERSARY" | "X-EVOLUTION-ANNIVERSARY" => c.anniversary = p.text_value(),
                "NOTE" => {
                    if !c.note.is_empty() {
                        c.note.push('\n');
                    }
                    c.note.push_str(&p.text_value());
                }
                "CATEGORIES" => c.categories.extend(p.list()),
                "PHOTO" => c.photo = decode_photo(p),
                "X-EVOLUTION-FILE-AS" => c.file_as = p.text_value(),
                "X-EVOLUTION-LIST" => c.is_list = p.value.eq_ignore_ascii_case("TRUE"),
                "REV" => c.rev = p.value.clone(),
                _ => c.extra.push(p.clone()),
            }
        }
        c
    }

    pub fn parse(text: &str) -> Option<Contact> {
        parse(text).map(|v| Contact::from_vcard(&v))
    }

    /// Builds a vCard 3.0 (the format EDS stores natively).
    pub fn to_vcard(&self) -> VCard {
        let mut props = vec![Property::new("VERSION", "3.0")];
        if !self.uid.is_empty() {
            props.push(Property::text("UID", &self.uid));
        }
        let display = self.display_name();
        props.push(Property::text("FN", &display));
        let n = &self.name;
        props.push(Property::new(
            "N",
            [&n.family, &n.given, &n.additional, &n.prefix, &n.suffix]
                .iter()
                .map(|s| escape_text(s))
                .collect::<Vec<_>>()
                .join(";"),
        ));
        let file_as = if self.file_as.trim().is_empty() {
            self.default_file_as()
        } else {
            self.file_as.clone()
        };
        props.push(Property::text("X-EVOLUTION-FILE-AS", &file_as));
        if !self.nickname.is_empty() {
            props.push(Property::text("NICKNAME", &self.nickname));
        }
        if !self.org.is_empty() || !self.org_unit.is_empty() {
            let mut v = escape_text(&self.org);
            if !self.org_unit.is_empty() {
                v.push(';');
                v.push_str(&escape_text(&self.org_unit));
            }
            props.push(Property::new("ORG", v));
        }
        if !self.title.is_empty() {
            props.push(Property::text("TITLE", &self.title));
        }
        if !self.role.is_empty() {
            props.push(Property::text("ROLE", &self.role));
        }
        let push_typed = |props: &mut Vec<Property>, name: &str, items: &[Typed]| {
            for t in items.iter().filter(|t| !t.value.trim().is_empty()) {
                let mut p = Property::text(name, t.value.trim());
                for ty in &t.types {
                    p.add_param("TYPE", ty);
                }
                if t.pref {
                    p.add_param("TYPE", "PREF");
                }
                props.push(p);
            }
        };
        push_typed(&mut props, "EMAIL", &self.emails);
        push_typed(&mut props, "TEL", &self.phones);
        for a in self.addresses.iter().filter(|a| !a.is_empty()) {
            let v = [
                &a.po_box,
                &a.extended,
                &a.street,
                &a.locality,
                &a.region,
                &a.postal_code,
                &a.country,
            ]
            .iter()
            .map(|s| escape_text(s.trim()))
            .collect::<Vec<_>>()
            .join(";");
            let mut p = Property::new("ADR", v);
            for ty in &a.types {
                p.add_param("TYPE", ty);
            }
            props.push(p);
        }
        push_typed(&mut props, "URL", &self.urls);
        push_typed(&mut props, "IMPP", &self.ims);
        if !self.birthday.is_empty() {
            props.push(Property::new("BDAY", self.birthday.clone()));
        }
        if !self.anniversary.is_empty() {
            props.push(Property::new(
                "X-EVOLUTION-ANNIVERSARY",
                self.anniversary.clone(),
            ));
        }
        if !self.note.is_empty() {
            props.push(Property::text("NOTE", &self.note));
        }
        if !self.categories.is_empty() {
            props.push(Property::new(
                "CATEGORIES",
                self.categories
                    .iter()
                    .map(|c| escape_text(c))
                    .collect::<Vec<_>>()
                    .join(","),
            ));
        }
        match &self.photo {
            Some(Photo::Inline { mime, data }) => {
                let mut p = Property::new(
                    "PHOTO",
                    base64::engine::general_purpose::STANDARD.encode(data),
                );
                p.add_param("ENCODING", "b");
                if let Some(m) = mime {
                    let sub = m.rsplit('/').next().unwrap_or(m).to_ascii_uppercase();
                    p.add_param("TYPE", &sub);
                }
                props.push(p);
            }
            Some(Photo::Uri(uri)) => {
                props.push(Property::new("PHOTO", uri.clone()).with_param("VALUE", "uri"))
            }
            None => {}
        }
        if self.is_list {
            props.push(Property::new("X-EVOLUTION-LIST", "TRUE"));
        }
        if !self.rev.is_empty() {
            props.push(Property::new("REV", self.rev.clone()));
        }
        props.extend(
            self.extra
                .iter()
                .filter(|p| !MANAGED.contains(&p.name.as_str()))
                .cloned(),
        );
        VCard { props }
    }

    pub fn to_vcard_string(&self) -> String {
        self.to_vcard().to_string_crlf()
    }

    /// Name shown in lists: FN, else composed N, else nickname, org, first
    /// email or phone.
    pub fn display_name(&self) -> String {
        let candidates = [
            self.full_name.trim().to_string(),
            self.name.display(),
            self.nickname.trim().to_string(),
            self.org.trim().to_string(),
            self.emails
                .first()
                .map(|e| e.value.clone())
                .unwrap_or_default(),
            self.phones
                .first()
                .map(|e| e.value.clone())
                .unwrap_or_default(),
        ];
        candidates
            .into_iter()
            .find(|s| !s.is_empty())
            .unwrap_or_default()
    }

    pub fn default_file_as(&self) -> String {
        match (self.name.family.is_empty(), self.name.given.is_empty()) {
            (false, false) => format!("{}, {}", self.name.family, self.name.given),
            (false, true) => self.name.family.clone(),
            (true, false) => self.name.given.clone(),
            (true, true) => self.display_name(),
        }
    }

    /// Key used to sort the contact list, case-folded. With `surname_first`
    /// the family name leads.
    pub fn sort_key(&self, surname_first: bool) -> String {
        let key = if surname_first && !self.name.family.is_empty() {
            format!("{} {}", self.name.family, self.name.given)
        } else if !surname_first && !self.name.given.is_empty() {
            format!("{} {}", self.name.given, self.name.family)
        } else {
            self.display_name()
        };
        key.trim().to_lowercase()
    }

    /// Whether any searchable field contains `needle` (case-insensitive).
    pub fn matches(&self, needle: &str) -> bool {
        let needle = needle.trim().to_lowercase();
        if needle.is_empty() {
            return true;
        }
        let digits: String = needle.chars().filter(|c| c.is_ascii_digit()).collect();
        let hay = [
            &self.display_name(),
            &self.nickname,
            &self.org,
            &self.title,
            &self.note,
            &self.name.family,
            &self.name.given,
        ];
        if hay.iter().any(|h| h.to_lowercase().contains(&needle)) {
            return true;
        }
        if self
            .emails
            .iter()
            .chain(self.urls.iter())
            .any(|e| e.value.to_lowercase().contains(&needle))
        {
            return true;
        }
        if digits.len() >= 3
            && self.phones.iter().any(|p| {
                p.value
                    .chars()
                    .filter(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .contains(&digits)
            })
        {
            return true;
        }
        self.addresses
            .iter()
            .any(|a| a.one_line().to_lowercase().contains(&needle))
    }

    /// The photo's image bytes: an inline photo, or a `file://` photo
    /// that EDS keeps in an address book's own `photos` directory. Other
    /// URIs (another local file, http…) are never read: a vCard from an
    /// import or a shared book could name any file. `None` when larger
    /// than [`MAX_PHOTO_BYTES`].
    pub fn photo_bytes(&self) -> Option<Vec<u8>> {
        match &self.photo {
            Some(Photo::Inline { data, .. }) => {
                (data.len() <= MAX_PHOTO_BYTES).then(|| data.clone())
            }
            Some(Photo::Uri(uri)) => read_eds_photo(uri, &eds_photo_roots()),
            None => None,
        }
    }

    /// For contacts coming from outside (an imported file): keeps only an
    /// inline photo within [`MAX_PHOTO_BYTES`] and drops linked ones, so
    /// the address book never stores a URI pointing at an arbitrary file.
    pub fn keep_inline_photo_only(&mut self) {
        if !matches!(&self.photo, Some(Photo::Inline { data, .. }) if data.len() <= MAX_PHOTO_BYTES)
        {
            self.photo = None;
        }
    }
}

/// Largest contact photo read from disk or kept from an import (10 MiB;
/// avatars are shown at most 96 px wide, and camera JPEGs fit easily).
pub const MAX_PHOTO_BYTES: usize = 10 * 1024 * 1024;

/// Where EDS keeps address-book data and caches: photos live in
/// `<root>/<book uid>/photos/` (local books under the data dir, CardDAV
/// and other cached books under the cache dir).
fn eds_photo_roots() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let dir = |var: &str, fallback: &str| {
        std::env::var_os(var)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home.as_ref().map(|h| h.join(fallback)))
    };
    [
        dir("XDG_DATA_HOME", ".local/share"),
        dir("XDG_CACHE_HOME", ".cache"),
    ]
    .into_iter()
    .flatten()
    .map(|d| d.join("evolution/addressbook"))
    .collect()
}

/// Reads a `file:///…` photo URI when it names a regular file directly in
/// `<root>/<book>/photos/` for one of `roots` (after resolving symlinks and
/// `..`), at most [`MAX_PHOTO_BYTES`] long.
fn read_eds_photo(uri: &str, roots: &[PathBuf]) -> Option<Vec<u8>> {
    use std::io::Read;
    use std::os::unix::ffi::OsStrExt;
    let path = uri.strip_prefix("file://")?;
    if !path.starts_with('/') {
        return None;
    }
    let path = Path::new(std::ffi::OsStr::from_bytes(&percent_decode(path)))
        .canonicalize()
        .ok()?;
    let inside = roots.iter().any(|root| {
        let Ok(root) = root.canonicalize() else {
            return false;
        };
        let Ok(rest) = path.strip_prefix(&root) else {
            return false;
        };
        let parts: Vec<_> = rest.components().collect();
        parts.len() == 3 && parts[1].as_os_str() == "photos"
    });
    if !inside {
        return None;
    }
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MAX_PHOTO_BYTES as u64 {
        return None;
    }
    let mut data = Vec::new();
    std::fs::File::open(&path)
        .ok()?
        .take(MAX_PHOTO_BYTES as u64 + 1)
        .read_to_end(&mut data)
        .ok()?;
    (data.len() <= MAX_PHOTO_BYTES).then_some(data)
}

/// Decodes `%XX` escapes over bytes (a `%` before anything else is kept).
fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let hex = |c: u8| (c as char).to_digit(16).map(|v| v as u8);
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

fn decode_photo(p: &Property) -> Option<Photo> {
    let value = p.value.trim();
    if let Some(rest) = value.strip_prefix("data:") {
        let (meta, data) = rest.split_once(',')?;
        let mime = meta
            .split(';')
            .next()
            .filter(|m| !m.is_empty())
            .map(str::to_string);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data.trim())
            .ok()?;
        return Some(Photo::Inline { mime, data: bytes });
    }
    let enc = p.param("ENCODING").unwrap_or("").to_ascii_lowercase();
    if enc == "b" || enc == "base64" {
        let clean: String = value.chars().filter(|c| !c.is_whitespace()).collect();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(clean)
            .ok()?;
        let mime = p.param("TYPE").map(|t| {
            if t.contains('/') {
                t.to_ascii_lowercase()
            } else {
                format!("image/{}", t.to_ascii_lowercase())
            }
        });
        return Some(Photo::Inline { mime, data: bytes });
    }
    if value.is_empty() {
        None
    } else {
        Some(Photo::Uri(value.to_string()))
    }
}

/// Human label for a phone's TYPE set (English, with a translation
/// context; show it with `helix_app_common::i18n::tr`, compare it only
/// with other labels from here).
pub fn phone_label(t: &Typed) -> &'static str {
    if t.has("CELL") {
        nc_!("phone number type", "Mobile")
    } else if t.has("FAX") {
        if t.has("WORK") {
            nc_!("phone number type", "Work Fax")
        } else {
            nc_!("phone number type", "Home Fax")
        }
    } else if t.has("PAGER") {
        nc_!("phone number type", "Pager")
    } else if t.has("HOME") {
        nc_!("phone number type", "Home")
    } else if t.has("WORK") {
        nc_!("phone number type", "Work")
    } else {
        nc_!("phone number type", "Other")
    }
}

/// Human label for an email / address / URL TYPE set.
pub fn generic_label(types: &[String]) -> &'static str {
    let has = |x: &str| types.iter().any(|t| t.eq_ignore_ascii_case(x));
    if has("HOME") {
        nc_!("email, address or website type", "Personal")
    } else if has("WORK") {
        nc_!("email, address or website type", "Work")
    } else {
        nc_!("email, address or website type", "Other")
    }
}

/// Phone type choices offered by editors: (label, TYPE values); the labels
/// are English, translate them where they are shown.
pub const PHONE_TYPES: &[(&str, &[&str])] = &[
    (nc_!("phone number type", "Mobile"), &["CELL"]),
    (nc_!("phone number type", "Home"), &["HOME", "VOICE"]),
    (nc_!("phone number type", "Work"), &["WORK", "VOICE"]),
    (nc_!("phone number type", "Home Fax"), &["HOME", "FAX"]),
    (nc_!("phone number type", "Work Fax"), &["WORK", "FAX"]),
    (nc_!("phone number type", "Pager"), &["PAGER"]),
    (nc_!("phone number type", "Other"), &["VOICE"]),
];

/// Email / address / URL type choices: (label, TYPE values).
pub const GENERIC_TYPES: &[(&str, &[&str])] = &[
    (
        nc_!("email, address or website type", "Personal"),
        &["HOME"],
    ),
    (nc_!("email, address or website type", "Work"), &["WORK"]),
    (nc_!("email, address or website type", "Other"), &["OTHER"]),
];

/// Index into [`PHONE_TYPES`] that best matches `t`.
pub fn phone_type_index(t: &Typed) -> usize {
    let label = phone_label(t);
    PHONE_TYPES
        .iter()
        .position(|(l, _)| *l == label)
        .unwrap_or(PHONE_TYPES.len() - 1)
}

/// Index into [`GENERIC_TYPES`] that best matches `types`.
pub fn generic_type_index(types: &[String]) -> usize {
    let label = generic_label(types);
    GENERIC_TYPES
        .iter()
        .position(|(l, _)| *l == label)
        .unwrap_or(GENERIC_TYPES.len() - 1)
}

/// Parses a vCard date (`YYYY-MM-DD`, `YYYYMMDD`, `--MMDD`, `--MM-DD`, with an
/// optional time part) into (year if known, month, day).
pub fn parse_date(v: &str) -> Option<(Option<i32>, u32, u32)> {
    let v = v.trim();
    let date = v.split('T').next()?;
    if let Some(md) = date.strip_prefix("--") {
        let md: String = md.chars().filter(|c| c.is_ascii_digit()).collect();
        if md.len() != 4 {
            return None;
        }
        return Some((None, md[..2].parse().ok()?, md[2..].parse().ok()?));
    }
    let digits: String = date.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() != 8 {
        return None;
    }
    let y: i32 = digits[..4].parse().ok()?;
    let m: u32 = digits[4..6].parse().ok()?;
    let d: u32 = digits[6..].parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some((Some(y), m, d))
}

/// Formats a date for BDAY (`YYYY-MM-DD`, vCard 3.0 style).
pub fn format_date(year: i32, month: u32, day: u32) -> String {
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:pas-id-1\r\nFN:Ada Lovelace\r\nN:Lovelace;Ada;Augusta;Lady;\r\nNICKNAME:Ada\r\nORG:Analytical Engines;R&D\r\nTITLE:Mathematician\r\nEMAIL;TYPE=WORK,PREF:ada@example.org\r\nEMAIL;TYPE=HOME:ada@home.example\r\nTEL;TYPE=CELL:+44 20 7946 0000\r\nTEL;TYPE=WORK;TYPE=FAX:+44 20 7946 0001\r\nADR;TYPE=HOME:;;12 St James\\, Square;London;;SW1Y;UK\r\nURL:https://example.org/ada\r\nBDAY:1815-12-10\r\nNOTE:First programmer\\nof sorts\r\nCATEGORIES:Friends,Science\r\nX-CUSTOM;FOO=bar:keep me\r\nitem1.X-ABLabel:custom label\r\nEND:VCARD\r\n";

    #[test]
    fn parse_contact_fields() {
        let c = Contact::parse(SAMPLE).unwrap();
        assert_eq!(c.uid, "pas-id-1");
        assert_eq!(c.full_name, "Ada Lovelace");
        assert_eq!(c.name.family, "Lovelace");
        assert_eq!(c.name.given, "Ada");
        assert_eq!(c.name.prefix, "Lady");
        assert_eq!(c.org, "Analytical Engines");
        assert_eq!(c.org_unit, "R&D");
        assert_eq!(c.emails.len(), 2);
        assert!(c.emails[0].pref);
        assert_eq!(c.emails[0].types, vec!["WORK"]);
        assert_eq!(
            phone_label(&c.phones[0]),
            nc_!("phone number type", "Mobile")
        );
        assert_eq!(
            phone_label(&c.phones[1]),
            nc_!("phone number type", "Work Fax")
        );
        assert_eq!(c.addresses[0].street, "12 St James, Square");
        assert_eq!(
            c.addresses[0].formatted(),
            "12 St James, Square\nLondon SW1Y\nUK"
        );
        assert_eq!(c.note, "First programmer\nof sorts");
        assert_eq!(c.categories, vec!["Friends", "Science"]);
        assert_eq!(c.extra.len(), 2);
        assert_eq!(parse_date(&c.birthday), Some((Some(1815), 12, 10)));
    }

    #[test]
    fn round_trip_preserves_everything() {
        let c = Contact::parse(SAMPLE).unwrap();
        let text = c.to_vcard_string();
        let again = Contact::parse(&text).unwrap();
        let mut expected = c.clone();
        expected.file_as = "Lovelace, Ada".into();
        assert_eq!(again, expected);
        assert!(text.contains("X-CUSTOM;FOO=bar:keep me\r\n"));
        assert!(text.contains("item1.X-ABLABEL:custom label"));
        assert!(text.starts_with("BEGIN:VCARD\r\nVERSION:3.0\r\n"));
    }

    #[test]
    fn display_name_fallbacks() {
        let mut c = Contact::default();
        c.emails.push(Typed::new(&["HOME"], "x@y.z"));
        assert_eq!(c.display_name(), "x@y.z");
        c.org = "ACME".into();
        assert_eq!(c.display_name(), "ACME");
        c.name = StructuredName::guess_from("Grace Brewster Hopper");
        assert_eq!(c.name.additional, "Brewster");
        assert_eq!(c.display_name(), "Grace Brewster Hopper");
        assert_eq!(c.default_file_as(), "Hopper, Grace");
    }

    #[test]
    fn sort_and_match() {
        let c = Contact::parse(SAMPLE).unwrap();
        assert_eq!(c.sort_key(false), "ada lovelace");
        assert_eq!(c.sort_key(true), "lovelace ada");
        assert!(c.matches("LOVE"));
        assert!(c.matches("home.example"));
        assert!(c.matches("7946 0000"));
        assert!(c.matches("london"));
        assert!(!c.matches("babbage"));
    }

    #[test]
    fn photo_inline_v3_and_v4() {
        let v3 = Contact::parse(
            "BEGIN:VCARD\nVERSION:3.0\nPHOTO;ENCODING=b;TYPE=PNG:aGVs\n bG8=\nEND:VCARD\n",
        )
        .unwrap();
        assert_eq!(
            v3.photo,
            Some(Photo::Inline {
                mime: Some("image/png".into()),
                data: b"hello".to_vec()
            })
        );
        let v4 = Contact::parse(
            "BEGIN:VCARD\nVERSION:4.0\nPHOTO:data:image/jpeg;base64,aGVsbG8=\nEND:VCARD\n",
        )
        .unwrap();
        assert_eq!(
            v4.photo,
            Some(Photo::Inline {
                mime: Some("image/jpeg".into()),
                data: b"hello".to_vec()
            })
        );
        let out = v4.to_vcard_string();
        assert!(out.contains("PHOTO;ENCODING=b;TYPE=JPEG:aGVsbG8="));
    }

    #[test]
    fn percent_decode_is_byte_safe() {
        assert_eq!(percent_decode("/a%20b/%e2%82%ac"), "/a b/€".as_bytes());
        // `%` before a multi-byte character or at the end: kept, no panic.
        assert_eq!(percent_decode("%é%"), "%é%".as_bytes());
        assert_eq!(percent_decode("x%4"), b"x%4");
        assert_eq!(percent_decode("%zz"), b"%zz");
    }

    #[test]
    fn photo_files_only_from_eds_photo_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("evolution/addressbook");
        let photos = root.join("book-1/photos");
        std::fs::create_dir_all(&photos).unwrap();
        let uri = |p: &Path| format!("file://{}", p.display()).replace(' ', "%20");
        let roots = vec![root.clone()];

        let own = photos.join("pas id-1.png");
        std::fs::write(&own, b"own photo").unwrap();
        assert_eq!(
            read_eds_photo(&uri(&own), &roots),
            Some(b"own photo".to_vec())
        );

        // Any other file: outside the root, beside `photos`, or reached
        // through `..` or a symlink.
        let secret = tmp.path().join("secret.txt");
        std::fs::write(&secret, b"private").unwrap();
        let beside = root.join("book-1/contacts.db");
        std::fs::write(&beside, b"db").unwrap();
        let dotdot = format!("{}/../../../secret.txt", uri(&photos));
        let link = photos.join("link.png");
        std::os::unix::fs::symlink(&secret, &link).unwrap();
        let zero = photos.join("zero.png");
        std::os::unix::fs::symlink("/dev/zero", &zero).unwrap();
        for bad in [uri(&secret), uri(&beside), dotdot, uri(&link), uri(&zero)] {
            assert_eq!(read_eds_photo(&bad, &roots), None, "{bad}");
        }
        // Relative, host-relative, non-file URIs and directories.
        for bad in [
            "file://secret.txt".to_string(),
            "https://example.org/a.png".into(),
            uri(&photos),
        ] {
            assert_eq!(read_eds_photo(&bad, &roots), None, "{bad}");
        }
        // Too large (sparse, so nothing is really written).
        let big = photos.join("big.png");
        let f = std::fs::File::create(&big).unwrap();
        f.set_len(MAX_PHOTO_BYTES as u64 + 1).unwrap();
        assert_eq!(read_eds_photo(&uri(&big), &roots), None);

        // `photo_bytes` never reads a file outside the EDS directories.
        let c = Contact {
            photo: Some(Photo::Uri(uri(&secret))),
            ..Default::default()
        };
        assert_eq!(c.photo_bytes(), None);
    }

    #[test]
    fn imported_photos_are_inline_only() {
        let card =
            "BEGIN:VCARD\nVERSION:3.0\nFN:X\nPHOTO;VALUE=uri:file:///etc/passwd\nEND:VCARD\n";
        let mut c = Contact::parse(card).unwrap();
        assert_eq!(c.photo, Some(Photo::Uri("file:///etc/passwd".into())));
        c.keep_inline_photo_only();
        assert_eq!(c.photo, None);

        let mut c = Contact::parse(
            "BEGIN:VCARD\nVERSION:4.0\nFN:X\nPHOTO:data:image/png;base64,aGVsbG8=\nEND:VCARD\n",
        )
        .unwrap();
        c.keep_inline_photo_only();
        assert_eq!(c.photo_bytes(), Some(b"hello".to_vec()));

        c.photo = Some(Photo::Inline {
            mime: None,
            data: vec![0; MAX_PHOTO_BYTES + 1],
        });
        assert_eq!(c.photo_bytes(), None);
        c.keep_inline_photo_only();
        assert_eq!(c.photo, None);
    }

    #[test]
    fn parse_many_and_v21() {
        let text = "BEGIN:VCARD\nVERSION:2.1\nN:Doe;John\nTEL;HOME;VOICE:555\nEND:VCARD\nBEGIN:VCARD\nVERSION:3.0\nFN:B\nEND:VCARD\n";
        let cards = parse_many(text);
        assert_eq!(cards.len(), 2);
        let c = Contact::from_vcard(&cards[0]);
        assert_eq!(c.display_name(), "John Doe");
        assert_eq!(phone_label(&c.phones[0]), nc_!("phone number type", "Home"));
    }

    #[test]
    fn dates() {
        assert_eq!(parse_date("--0517"), Some((None, 5, 17)));
        assert_eq!(parse_date("19900517"), Some((Some(1990), 5, 17)));
        assert_eq!(
            parse_date("1990-05-17T00:00:00Z"),
            Some((Some(1990), 5, 17))
        );
        assert_eq!(parse_date("1990-13-17"), None);
        assert_eq!(format_date(1990, 5, 7), "1990-05-07");
    }

    #[test]
    fn type_indices() {
        assert_eq!(
            PHONE_TYPES[phone_type_index(&Typed::new(&["CELL"], "1"))].0,
            nc_!("phone number type", "Mobile")
        );
        assert_eq!(
            GENERIC_TYPES[generic_type_index(&["WORK".into()])].0,
            nc_!("email, address or website type", "Work")
        );
        assert_eq!(
            GENERIC_TYPES[generic_type_index(&[])].0,
            nc_!("email, address or website type", "Other")
        );
    }
}
