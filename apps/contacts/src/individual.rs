//! Individuals: contacts from all address books, with linked contacts
//! merged into one entry, plus sorting, grouping and filtering.

use crate::links::{Key, Links};
use helix_eds::vcard::{Address, Contact, Typed};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct Individual {
    /// Members in a stable order; the first is the primary.
    pub members: Vec<Key>,
    /// Merged view used for display.
    pub contact: Contact,
}

impl Individual {
    pub fn id(&self) -> String {
        let (b, u) = &self.members[0];
        format!("{b}\u{1f}{u}")
    }

    pub fn is_linked(&self) -> bool {
        self.members.len() > 1
    }

    pub fn contains_uid(&self, uid: &str) -> bool {
        self.members.iter().any(|(_, u)| u == uid)
    }
}

fn push_unique_typed(into: &mut Vec<Typed>, items: &[Typed], norm: impl Fn(&str) -> String) {
    for t in items {
        if !into.iter().any(|x| norm(&x.value) == norm(&t.value)) {
            into.push(t.clone());
        }
    }
}

fn phone_norm(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_digit() || *c == '+').collect()
}

/// Merges contacts for display: scalar fields come from the first member
/// that has them; multi-valued fields are the de-duplicated union.
pub fn merge(contacts: &[&Contact]) -> Contact {
    let mut out = match contacts.first() {
        Some(c) => (*c).clone(),
        None => return Contact::default(),
    };
    for c in &contacts[1..] {
        let fill = |dst: &mut String, src: &str| {
            if dst.trim().is_empty() {
                *dst = src.to_string();
            }
        };
        fill(&mut out.full_name, &c.full_name);
        if out.name.is_empty() {
            out.name = c.name.clone();
        }
        fill(&mut out.nickname, &c.nickname);
        fill(&mut out.org, &c.org);
        fill(&mut out.org_unit, &c.org_unit);
        fill(&mut out.title, &c.title);
        fill(&mut out.birthday, &c.birthday);
        fill(&mut out.anniversary, &c.anniversary);
        if !c.note.is_empty() && !out.note.contains(&c.note) {
            if !out.note.is_empty() {
                out.note.push_str("\n\n");
            }
            out.note.push_str(&c.note);
        }
        if out.photo.is_none() {
            out.photo = c.photo.clone();
        }
        push_unique_typed(&mut out.emails, &c.emails, |s| s.trim().to_lowercase());
        push_unique_typed(&mut out.phones, &c.phones, phone_norm);
        push_unique_typed(&mut out.urls, &c.urls, |s| s.trim().trim_end_matches('/').to_lowercase());
        push_unique_typed(&mut out.ims, &c.ims, |s| s.trim().to_lowercase());
        for a in &c.addresses {
            if !out.addresses.iter().any(|x: &Address| x.one_line().eq_ignore_ascii_case(&a.one_line())) {
                out.addresses.push(a.clone());
            }
        }
        for cat in &c.categories {
            if !out.categories.contains(cat) {
                out.categories.push(cat.clone());
            }
        }
    }
    out
}

/// Builds individuals from all contacts (`book uid → contacts`) and links.
/// Contact lists (`X-EVOLUTION-LIST`) are skipped.
pub fn build(books: &HashMap<String, HashMap<String, Contact>>, links: &Links) -> Vec<Individual> {
    let mut out = Vec::new();
    let mut used: std::collections::HashSet<Key> = Default::default();
    for group in &links.groups {
        let members: Vec<Key> =
            group.iter().filter(|(b, u)| books.get(b).is_some_and(|m| m.contains_key(u))).cloned().collect();
        if members.is_empty() {
            continue;
        }
        let contacts: Vec<&Contact> = members.iter().map(|(b, u)| &books[b][u]).collect();
        let contact = merge(&contacts);
        used.extend(members.iter().cloned());
        out.push(Individual { members, contact });
    }
    let mut book_ids: Vec<&String> = books.keys().collect();
    book_ids.sort();
    for b in book_ids {
        let mut uids: Vec<&String> = books[b].keys().collect();
        uids.sort();
        for u in uids {
            let key = (b.clone(), u.clone());
            let c = &books[b][u];
            if used.contains(&key) || c.is_list {
                continue;
            }
            out.push(Individual { members: vec![key], contact: c.clone() });
        }
    }
    out
}

/// Sorts by the display sort key; ties broken by id for stability.
pub fn sort(list: &mut [Individual], surname_first: bool) {
    list.sort_by_cached_key(|i| (i.contact.sort_key(surname_first), i.id()));
}

/// Filters by a search string.
pub fn filter<'a>(list: &'a [Individual], query: &str) -> Vec<&'a Individual> {
    list.iter().filter(|i| i.contact.matches(query)).collect()
}

/// Section header letter for a sort key: the upper-cased first letter, or
/// `#` for digits / symbols.
pub fn group_letter(sort_key: &str) -> String {
    match sort_key.chars().next() {
        Some(c) if c.is_alphabetic() => c.to_uppercase().collect(),
        _ => "#".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_eds::vcard::StructuredName;

    fn contact(name: &str, email: &str) -> Contact {
        let mut c = Contact { name: StructuredName::guess_from(name), full_name: name.into(), ..Default::default() };
        if !email.is_empty() {
            c.emails.push(Typed::new(&["HOME"], email));
        }
        c
    }

    fn books() -> HashMap<String, HashMap<String, Contact>> {
        let mut a = HashMap::new();
        a.insert("1".to_string(), contact("Zoe Adams", "zoe@a.org"));
        a.insert("2".to_string(), contact("adam Young", ""));
        let mut list = contact("Team", "");
        list.is_list = true;
        a.insert("3".to_string(), list);
        let mut b = HashMap::new();
        let mut z = contact("", "ZOE@a.org");
        z.phones.push(Typed::new(&["CELL"], "+1 555"));
        z.title = "CTO".into();
        b.insert("9".to_string(), z);
        b.insert("8".to_string(), contact("4 Non Blondes", ""));
        let mut m = HashMap::new();
        m.insert("local".to_string(), a);
        m.insert("google".to_string(), b);
        m
    }

    #[test]
    fn build_merges_links_and_skips_lists() {
        let mut links = Links::default();
        links.link(&[("local".into(), "1".into()), ("google".into(), "9".into())]);
        let list = build(&books(), &links);
        assert_eq!(list.len(), 3);
        let zoe = list.iter().find(|i| i.is_linked()).unwrap();
        assert_eq!(zoe.contact.display_name(), "Zoe Adams");
        assert_eq!(zoe.contact.emails.len(), 1, "case-insensitive email de-dup");
        assert_eq!(zoe.contact.phones.len(), 1);
        assert_eq!(zoe.contact.title, "CTO");
        assert!(zoe.contains_uid("9"));
    }

    #[test]
    fn sorting_and_letters() {
        let mut list = build(&books(), &Links::default());
        sort(&mut list, false);
        let names: Vec<String> = list.iter().map(|i| i.contact.display_name()).collect();
        assert_eq!(names, ["4 Non Blondes", "adam Young", "Zoe Adams", "ZOE@a.org"]);
        assert_eq!(group_letter(&list[0].contact.sort_key(false)), "#");
        assert_eq!(group_letter("adam young"), "A");
        assert_eq!(group_letter("éclair"), "É");
        sort(&mut list, true);
        let first_named: Vec<String> = list.iter().map(|i| i.contact.sort_key(true)).collect();
        let mut sorted = first_named.clone();
        sorted.sort();
        assert_eq!(first_named, sorted);
        assert!(first_named.contains(&"adams zoe".to_string()));
    }

    #[test]
    fn filtering() {
        let list = build(&books(), &Links::default());
        assert_eq!(filter(&list, "zoe").len(), 2);
        assert_eq!(filter(&list, "").len(), list.len());
        assert_eq!(filter(&list, "blondes").len(), 1);
    }

    #[test]
    fn merge_notes_and_scalars() {
        let mut a = contact("A B", "");
        a.note = "one".into();
        let mut b = contact("", "");
        b.note = "two".into();
        b.org = "Org".into();
        let m = merge(&[&a, &b]);
        assert_eq!(m.note, "one\n\ntwo");
        assert_eq!(m.org, "Org");
        assert_eq!(m.full_name, "A B");
    }
}
