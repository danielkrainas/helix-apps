//! EDS data sources (address books, calendars, accounts) as described by the
//! source registry (`org.gnome.evolution.dataserver.Sources5`).

use crate::keyfile::{user_languages, KeyFile};

pub const GROUP_SOURCE: &str = "Data Source";
pub const GROUP_ADDRESS_BOOK: &str = "Address Book";
pub const GROUP_CALENDAR: &str = "Calendar";
pub const GROUP_TASK_LIST: &str = "Task List";
pub const GROUP_MEMO_LIST: &str = "Memo List";
pub const GROUP_COLLECTION: &str = "Collection";

/// The kind of data a source holds (from its key-file extension groups).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceKind {
    AddressBook,
    Calendar,
    TaskList,
    MemoList,
    Collection,
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub uid: String,
    /// D-Bus object path in the registry (empty for sources not yet created).
    pub object_path: String,
    pub data: KeyFile,
    /// Has the `Source.Writable` interface (its data can be rewritten).
    pub writable: bool,
    /// Has the `Source.Removable` interface.
    pub removable: bool,
}

impl Source {
    pub fn from_data(uid: &str, object_path: &str, data: &str) -> Source {
        Source {
            uid: uid.to_string(),
            object_path: object_path.to_string(),
            data: KeyFile::parse(data),
            writable: false,
            removable: false,
        }
    }

    pub fn display_name(&self) -> String {
        let langs = user_languages();
        self.data
            .get_locale(GROUP_SOURCE, "DisplayName", &langs)
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.uid)
            .to_string()
    }

    pub fn parent(&self) -> Option<&str> {
        self.data.get(GROUP_SOURCE, "Parent").filter(|p| !p.is_empty())
    }

    pub fn enabled(&self) -> bool {
        self.data.get_bool(GROUP_SOURCE, "Enabled").unwrap_or(true)
    }

    pub fn kinds(&self) -> Vec<SourceKind> {
        let mut out = Vec::new();
        for (group, kind) in [
            (GROUP_ADDRESS_BOOK, SourceKind::AddressBook),
            (GROUP_CALENDAR, SourceKind::Calendar),
            (GROUP_TASK_LIST, SourceKind::TaskList),
            (GROUP_MEMO_LIST, SourceKind::MemoList),
            (GROUP_COLLECTION, SourceKind::Collection),
        ] {
            if self.data.has_group(group) {
                out.push(kind);
            }
        }
        if out.is_empty() {
            out.push(SourceKind::Other);
        }
        out
    }

    pub fn is(&self, kind: SourceKind) -> bool {
        self.kinds().contains(&kind)
    }

    fn group_for(kind: SourceKind) -> &'static str {
        match kind {
            SourceKind::AddressBook => GROUP_ADDRESS_BOOK,
            SourceKind::Calendar => GROUP_CALENDAR,
            SourceKind::TaskList => GROUP_TASK_LIST,
            SourceKind::MemoList => GROUP_MEMO_LIST,
            SourceKind::Collection => GROUP_COLLECTION,
            SourceKind::Other => GROUP_SOURCE,
        }
    }

    pub fn backend_name(&self, kind: SourceKind) -> Option<&str> {
        self.data.get(Self::group_for(kind), "BackendName")
    }

    /// Calendar colour (`#rrggbb`), if set.
    pub fn color(&self) -> Option<&str> {
        self.data.get(GROUP_CALENDAR, "Color").filter(|c| !c.is_empty())
    }

    /// Whether the calendar is shown ("Selected" in the calendar extension).
    pub fn selected(&self) -> bool {
        self.data.get_bool(GROUP_CALENDAR, "Selected").unwrap_or(true)
    }

    pub fn set_selected(&mut self, selected: bool) {
        self.data.set_bool(GROUP_CALENDAR, "Selected", selected);
    }

    pub fn set_color(&mut self, color: &str) {
        self.data.set(GROUP_CALENDAR, "Color", color);
    }

    pub fn set_display_name(&mut self, name: &str) {
        self.data.set_unlocalized(GROUP_SOURCE, "DisplayName", name);
    }

    /// Serialized key-file data, suitable for `Source.Writable.Write` or
    /// `SourceManager.CreateSources`.
    pub fn data_string(&self) -> String {
        self.data.to_string()
    }
}

/// Key-file data for a new local ("On This Computer") calendar.
pub fn new_local_calendar(display_name: &str, color: &str) -> String {
    let mut kf = KeyFile::default();
    kf.set(GROUP_SOURCE, "DisplayName", display_name);
    kf.set(GROUP_SOURCE, "Enabled", "true");
    kf.set(GROUP_SOURCE, "Parent", "local-stub");
    kf.set(GROUP_CALENDAR, "BackendName", "local");
    kf.set(GROUP_CALENDAR, "Color", color);
    kf.set(GROUP_CALENDAR, "Selected", "true");
    kf.to_string()
}

/// Key-file data for a new local address book.
pub fn new_local_address_book(display_name: &str) -> String {
    let mut kf = KeyFile::default();
    kf.set(GROUP_SOURCE, "DisplayName", display_name);
    kf.set(GROUP_SOURCE, "Enabled", "true");
    kf.set(GROUP_SOURCE, "Parent", "local-stub");
    kf.set(GROUP_ADDRESS_BOOK, "BackendName", "local");
    kf.to_string()
}

/// A fresh source UID (40 hex chars, like EDS generates).
pub fn new_uid() -> String {
    crate::new_id()
}

/// Label for the account a source belongs to (its parent's display name, or
/// its own when it has no parent).
pub fn account_label(source: &Source, all: &[Source]) -> String {
    match source.parent().and_then(|p| all.iter().find(|s| s.uid == p)) {
        Some(parent) => parent.display_name(),
        None => source.display_name(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_source_fields() {
        let s = Source::from_data(
            "abc",
            "/p",
            "[Data Source]\nDisplayName=Work\nParent=google-stub\n\n[Calendar]\nBackendName=caldav\nColor=#ff0000\nSelected=false\n",
        );
        assert_eq!(s.display_name(), "Work");
        assert_eq!(s.parent(), Some("google-stub"));
        assert_eq!(s.kinds(), vec![SourceKind::Calendar]);
        assert_eq!(s.backend_name(SourceKind::Calendar), Some("caldav"));
        assert_eq!(s.color(), Some("#ff0000"));
        assert!(!s.selected());
        assert!(s.enabled());
    }

    #[test]
    fn edit_and_serialize() {
        let mut s = Source::from_data("abc", "/p", &new_local_calendar("Home", "#00ff00"));
        s.set_selected(false);
        s.set_color("#123456");
        s.set_display_name("House");
        let again = Source::from_data("abc", "/p", &s.data_string());
        assert_eq!(again.display_name(), "House");
        assert_eq!(again.color(), Some("#123456"));
        assert!(!again.selected());
        assert_eq!(again.parent(), Some("local-stub"));
    }

    #[test]
    fn account_label_uses_parent() {
        let parent = Source::from_data("local-stub", "/a", "[Data Source]\nDisplayName=On This Computer\n");
        let child = Source::from_data("x", "/b", &new_local_address_book("Friends"));
        assert!(child.is(SourceKind::AddressBook));
        assert_eq!(account_label(&child, &[parent.clone(), child.clone()]), "On This Computer");
        assert_eq!(account_label(&parent, std::slice::from_ref(&parent)), "On This Computer");
    }
}
