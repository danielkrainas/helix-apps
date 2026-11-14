//! Read-only import of links made by GNOME Contacts (libfolks).
//!
//! GNOME Contacts links contacts through libfolks, which keeps no link table
//! of its own for address-book contacts. Instead a link is expressed through
//! *linkable properties* stored on a persona in the primary store (normally
//! EDS' `system-address-book`):
//!
//! * `X-FOLKS-CONTACTS-IDS` — the persona's "local IDs". For EDS personas a
//!   local ID is the persona IID, `<source-uid>:<contact-uid>`. Every contact
//!   listed there (and the contact carrying the attribute) form one
//!   individual. Values may be repeated attributes or one attribute with a
//!   `,`/`;` separated list.
//! * `X-FOLKS-ANTI-LINKS` — persona UIDs (`eds:<source-uid>:<contact-uid>`,
//!   `:` inside components escaped as `\:`) that must *not* be linked to the
//!   contact carrying the attribute.
//!
//! Folks' key-file backend (`$XDG_DATA_HOME/folks/relationships.ini`) is a
//! GKeyFile whose groups are personas and whose keys are IM protocols /
//! web services with `;`-separated address lists (`__`-prefixed keys are
//! metadata such as `__alias`). Folks links every persona that shares such an
//! address; we mirror that for EDS contacts (matching their IM addresses or
//! IIDs). This file only matters when the key-file store was made primary.
//!
//! Nothing here is ever written: Helix's own links live in `links.json`.

use crate::links::Key;
use helix_eds::contentline::{split_unescaped, unescape_text};
use helix_eds::vcard::Contact;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const CONTACTS_IDS: &str = "X-FOLKS-CONTACTS-IDS";
const ANTI_LINKS: &str = "X-FOLKS-ANTI-LINKS";

/// vCard properties EDS uses for IM addresses (besides `IMPP`).
const IM_PROPS: &[&str] = &[
    "X-AIM",
    "X-JABBER",
    "X-YAHOO",
    "X-MSN",
    "X-ICQ",
    "X-GROUPWISE",
    "X-GADUGADU",
    "X-SKYPE",
    "X-GOOGLE-TALK",
    "X-TWITTER",
    "X-MATRIX",
];

/// Splits an attribute value into its list items (`,` or `;` separated,
/// vCard-escaped).
fn values(raw: &str) -> Vec<String> {
    split_unescaped(raw, ';')
        .iter()
        .flat_map(|s| split_unescaped(s, ','))
        .map(|s| unescape_text(&s).trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn attr_values(c: &Contact, name: &str) -> Vec<String> {
    c.extra
        .iter()
        .filter(|p| p.name.eq_ignore_ascii_case(name))
        .flat_map(|p| values(&p.value))
        .collect()
}

/// Splits on `:` not escaped by a backslash and removes the escapes.
fn split_colons(s: &str) -> Vec<String> {
    split_unescaped(s, ':')
        .iter()
        .map(|p| p.replace("\\:", ":").replace("\\\\", "\\"))
        .collect()
}

/// Parses a folks local ID for an EDS persona: `<source-uid>:<contact-uid>`.
/// A full persona UID (`eds:<source>:<contact>`) is accepted as well.
pub fn parse_local_id(id: &str) -> Option<Key> {
    if let Some(key) = parse_persona_uid(id) {
        return Some(key);
    }
    let (book, uid) = id.split_once(':')?;
    if book.is_empty() || uid.is_empty() {
        return None;
    }
    Some((book.to_string(), uid.to_string()))
}

/// Parses a folks persona UID of the EDS backend: `eds:<source>:<contact>`.
pub fn parse_persona_uid(id: &str) -> Option<Key> {
    let parts = split_colons(id);
    match parts.as_slice() {
        [backend, book, uid] if backend == "eds" && !book.is_empty() && !uid.is_empty() => {
            Some((book.clone(), uid.clone()))
        }
        _ => None,
    }
}

/// Link groups expressed by `X-FOLKS-CONTACTS-IDS` on the given contacts
/// (`book uid → contacts`). Groups may overlap; callers merge them.
pub fn groups_from_contacts(books: &HashMap<String, HashMap<String, Contact>>) -> Vec<Vec<Key>> {
    let mut out = Vec::new();
    let mut book_ids: Vec<&String> = books.keys().collect();
    book_ids.sort();
    for book in book_ids {
        let mut uids: Vec<&String> = books[book].keys().collect();
        uids.sort();
        for uid in uids {
            let c = &books[book][uid];
            let ids = attr_values(c, CONTACTS_IDS);
            if ids.is_empty() {
                continue;
            }
            let me: Key = (book.clone(), uid.clone());
            let anti: HashSet<Key> = attr_values(c, ANTI_LINKS)
                .iter()
                .filter_map(|v| parse_persona_uid(v).or_else(|| parse_local_id(v)))
                .collect();
            let mut group = vec![me.clone()];
            for key in ids.iter().filter_map(|id| parse_local_id(id)) {
                if !anti.contains(&key) && !group.contains(&key) {
                    group.push(key);
                }
            }
            if group.len() > 1 {
                out.push(group);
            }
        }
    }
    out
}

/// One persona of folks' key-file backend: its linkable addresses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyFilePersona {
    pub id: String,
    /// Lower-cased addresses from all non-metadata keys.
    pub addresses: Vec<String>,
}

/// Unescapes a GKeyFile string (`\s`, `\n`, `\t`, `\r`, `\\`, `\;`).
fn keyfile_unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(o) => out.push(o),
            None => {}
        }
    }
    out
}

/// Parses folks' `relationships.ini`.
pub fn parse_relationships(text: &str) -> Vec<KeyFilePersona> {
    let mut out: Vec<KeyFilePersona> = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some(name) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            out.push(KeyFilePersona {
                id: name.to_string(),
                addresses: Vec::new(),
            });
            continue;
        }
        let (Some(p), Some((key, value))) = (out.last_mut(), t.split_once('=')) else {
            continue;
        };
        let key = key.trim();
        if key.starts_with("__") || key.contains('[') {
            continue;
        }
        for item in split_unescaped(value.trim(), ';') {
            let v = keyfile_unescape(&item).trim().to_lowercase();
            if !v.is_empty() && !p.addresses.contains(&v) {
                p.addresses.push(v);
            }
        }
    }
    out.retain(|p| !p.addresses.is_empty());
    out
}

/// Default location of folks' key-file store.
pub fn relationships_path() -> PathBuf {
    gtk::glib::user_data_dir()
        .join("folks")
        .join("relationships.ini")
}

pub fn load_relationships(path: &Path) -> Vec<KeyFilePersona> {
    std::fs::read_to_string(path)
        .map(|t| parse_relationships(&t))
        .unwrap_or_default()
}

/// IM addresses of a contact, lower-cased and without URI scheme.
fn im_addresses(c: &Contact) -> Vec<String> {
    let strip = |s: &str| {
        let s = s.trim();
        let s = match s.split_once(':') {
            Some((scheme, rest)) if !scheme.contains('@') && !rest.starts_with("//") => rest,
            _ => s,
        };
        s.to_lowercase()
    };
    let mut out: Vec<String> = c.ims.iter().map(|t| strip(&t.value)).collect();
    for name in IM_PROPS {
        out.extend(attr_values(c, name).iter().map(|v| strip(v)));
    }
    out.retain(|s| !s.is_empty());
    out
}

/// Link groups implied by key-file personas: EDS contacts sharing one of a
/// persona's addresses (IM address or IID) are linked.
pub fn groups_from_keyfile(
    personas: &[KeyFilePersona],
    books: &HashMap<String, HashMap<String, Contact>>,
) -> Vec<Vec<Key>> {
    if personas.is_empty() {
        return Vec::new();
    }
    let mut index: HashMap<String, Vec<Key>> = HashMap::new();
    let mut book_ids: Vec<&String> = books.keys().collect();
    book_ids.sort();
    for book in book_ids {
        let mut uids: Vec<&String> = books[book].keys().collect();
        uids.sort();
        for uid in uids {
            let key: Key = (book.clone(), uid.clone());
            index
                .entry(format!("{book}:{uid}").to_lowercase())
                .or_default()
                .push(key.clone());
            for a in im_addresses(&books[book][uid]) {
                index.entry(a).or_default().push(key.clone());
            }
        }
    }
    let mut out = Vec::new();
    for p in personas {
        let mut group: Vec<Key> = Vec::new();
        for a in &p.addresses {
            for k in index.get(a).into_iter().flatten() {
                if !group.contains(k) {
                    group.push(k.clone());
                }
            }
        }
        if group.len() > 1 {
            out.push(group);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(b: &str, u: &str) -> Key {
        (b.into(), u.into())
    }

    fn contact(text: &str) -> Contact {
        Contact::parse(text).expect("vcard")
    }

    fn books(entries: &[(&str, &str, &str)]) -> HashMap<String, HashMap<String, Contact>> {
        let mut m: HashMap<String, HashMap<String, Contact>> = HashMap::new();
        for (b, u, extra) in entries {
            let c = contact(&format!(
                "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:{u}\r\nFN:{u}\r\n{extra}END:VCARD\r\n"
            ));
            m.entry(b.to_string()).or_default().insert(u.to_string(), c);
        }
        m
    }

    #[test]
    fn local_ids() {
        assert_eq!(
            parse_local_id("system-address-book:pas-id-1"),
            Some(k("system-address-book", "pas-id-1"))
        );
        assert_eq!(
            parse_local_id("eds:abc123:pas-id-2"),
            Some(k("abc123", "pas-id-2"))
        );
        assert_eq!(parse_local_id("eds:a\\:b:c"), Some(k("a:b", "c")));
        assert_eq!(parse_local_id("nocolon"), None);
        assert_eq!(parse_local_id(":x"), None);
        assert_eq!(parse_persona_uid("key-file::42"), None);
    }

    #[test]
    fn contacts_ids_build_groups() {
        let b = books(&[
            ("system-address-book", "p1", "X-FOLKS-CONTACTS-IDS:google:g7,carddav:c3\r\n"),
            ("google", "g7", ""),
            ("carddav", "c3", ""),
            ("system-address-book", "p2", "X-FOLKS-CONTACTS-IDS:google:g8\r\nX-FOLKS-CONTACTS-IDS:google:g9\r\nX-FOLKS-ANTI-LINKS:eds:google:g9\r\n"),
            ("system-address-book", "p3", "X-FOLKS-CONTACTS-IDS:system-address-book:p3\r\n"),
        ]);
        let mut g = groups_from_contacts(&b);
        g.sort();
        assert_eq!(
            g,
            vec![
                vec![
                    k("system-address-book", "p1"),
                    k("google", "g7"),
                    k("carddav", "c3")
                ],
                vec![k("system-address-book", "p2"), k("google", "g8")],
            ]
        );
    }

    #[test]
    fn semicolon_separated_ids() {
        assert_eq!(values("a:1;b:2\\,x , c:3"), vec!["a:1", "b:2,x", "c:3"]);
    }

    #[test]
    fn keyfile_parse_and_link() {
        let text = "# folks\n[0]\n__alias=Ada\njabber=Ada@Example.org;ada2@example.org;\n\n[1]\nmsn=\n[2]\nlocal-ids=sys:p1;goo:g1\n";
        let personas = parse_relationships(text);
        assert_eq!(personas.len(), 2);
        assert_eq!(
            personas[0].addresses,
            vec!["ada@example.org", "ada2@example.org"]
        );
        let b = books(&[
            ("sys", "p1", "X-JABBER:ada@example.org\r\n"),
            ("goo", "g1", "IMPP:xmpp:ADA2@example.org\r\n"),
            ("goo", "g2", ""),
        ]);
        let mut groups = groups_from_keyfile(&personas, &b);
        for g in &mut groups {
            g.sort();
        }
        assert_eq!(
            groups,
            vec![
                vec![k("goo", "g1"), k("sys", "p1")],
                vec![k("goo", "g1"), k("sys", "p1")]
            ]
        );
        assert!(groups_from_keyfile(&[], &b).is_empty());
        assert!(load_relationships(Path::new("/nonexistent/relationships.ini")).is_empty());
    }
}
