//! Sidebar categories and entry classification.

use crate::journal::{Entry, QueryArgs};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Important,
    All,
    Applications,
    System,
    Security,
    Hardware,
}

impl Category {
    pub const ALL: [Category; 6] = [
        Category::Important,
        Category::All,
        Category::Applications,
        Category::System,
        Category::Security,
        Category::Hardware,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Important => "Important",
            Category::All => "All",
            Category::Applications => "Applications",
            Category::System => "System",
            Category::Security => "Security",
            Category::Hardware => "Hardware",
        }
    }

    /// Whether `entry` belongs to this category. `uid` is the current user.
    pub fn matches(self, entry: &Entry, uid: u32) -> bool {
        match self {
            Category::Important => entry.priority() <= 3,
            Category::All => true,
            Category::Applications => is_application(entry, uid),
            Category::System => entry.transport() != "kernel" && !is_application(entry, uid),
            Category::Security => is_security(entry),
            Category::Hardware => entry.transport() == "kernel",
        }
    }

    /// Narrows `q` so journalctl only emits candidate entries.
    pub fn prefilter(self, q: &mut QueryArgs, uid: u32) {
        match self {
            Category::Important => q.max_priority = Some(3),
            Category::All | Category::System => {}
            Category::Applications => {
                q.any_of = vec![("_UID".into(), uid.to_string()), ("_SYSTEMD_OWNER_UID".into(), uid.to_string())];
            }
            Category::Security => {
                q.any_of = vec![
                    ("_TRANSPORT".into(), "audit".into()),
                    ("SYSLOG_FACILITY".into(), "4".into()),
                    ("SYSLOG_FACILITY".into(), "10".into()),
                ];
            }
            Category::Hardware => q.any_of = vec![("_TRANSPORT".into(), "kernel".into())],
        }
    }
}

fn is_security(entry: &Entry) -> bool {
    entry.transport() == "audit" || entry.get_all("SYSLOG_FACILITY").any(|f| f == "4" || f == "10")
}

fn is_application(entry: &Entry, uid: u32) -> bool {
    if matches!(entry.transport(), "kernel" | "audit") {
        return false;
    }
    let uid_s = uid.to_string();
    let owned = entry.get("_UID") == Some(uid_s.as_str()) || entry.get("_SYSTEMD_OWNER_UID") == Some(uid_s.as_str());
    if !owned {
        return false;
    }
    entry.has("_SYSTEMD_USER_UNIT") || (uid != 0 && entry.has("SYSLOG_IDENTIFIER"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<Entry> {
        include_str!("../tests/fixtures/journal.jsonl")
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| Entry::parse(l).unwrap())
            .collect()
    }

    fn members(c: Category, uid: u32) -> Vec<usize> {
        fixture().iter().enumerate().filter(|(_, e)| c.matches(e, uid)).map(|(i, _)| i).collect()
    }

    #[test]
    fn classification() {
        assert_eq!(members(Category::All, 1000), [0, 1, 2, 3, 4, 5, 6]);
        assert_eq!(members(Category::Important, 1000), [1, 5]);
        assert_eq!(members(Category::Applications, 1000), [0, 1, 6]);
        assert_eq!(members(Category::System, 1000), [3, 4, 5]);
        assert_eq!(members(Category::Security, 1000), [3, 4]);
        assert_eq!(members(Category::Hardware, 1000), [2]);
    }

    #[test]
    fn other_user_entries_are_system() {
        assert_eq!(members(Category::Applications, 1001), Vec::<usize>::new());
        assert_eq!(members(Category::System, 1001), [0, 1, 3, 4, 5, 6]);
    }

    #[test]
    fn root_applications_need_user_unit() {
        let e = Entry::from_fields([("_UID", "0"), ("SYSLOG_IDENTIFIER", "sshd"), ("_TRANSPORT", "syslog")]);
        assert!(!Category::Applications.matches(&e, 0));
        let e = Entry::from_fields([("_UID", "0"), ("_SYSTEMD_USER_UNIT", "x.service"), ("_TRANSPORT", "journal")]);
        assert!(Category::Applications.matches(&e, 0));
    }

    #[test]
    fn facility_auth() {
        let e = Entry::from_fields([("SYSLOG_FACILITY", "4")]);
        assert!(Category::Security.matches(&e, 1000));
        let e = Entry::from_fields([("SYSLOG_FACILITY", "3")]);
        assert!(!Category::Security.matches(&e, 1000));
    }

    #[test]
    fn prefilters() {
        let mut q = QueryArgs::default();
        Category::Security.prefilter(&mut q, 1000);
        assert_eq!(q.any_of.len(), 3);
        let mut q = QueryArgs::default();
        Category::Important.prefilter(&mut q, 1000);
        assert_eq!(q.max_priority, Some(3));
        let mut q = QueryArgs::default();
        Category::Applications.prefilter(&mut q, 1000);
        assert_eq!(q.to_args()[4..], ["_UID=1000", "+", "_SYSTEMD_OWNER_UID=1000"]);
        // Every fixture entry the client-side rule accepts must survive the
        // pre-filter.
        for c in Category::ALL {
            let mut q = QueryArgs::default();
            c.prefilter(&mut q, 1000);
            for e in fixture().iter().filter(|e| c.matches(e, 1000)) {
                let ok_prio = q.max_priority.is_none_or(|p| e.priority() <= p);
                let ok_match = q.any_of.is_empty() || q.any_of.iter().any(|(k, v)| e.get_all(k).any(|x| x == v));
                assert!(ok_prio && ok_match, "{c:?} pre-filter drops {e:?}");
            }
        }
    }
}
