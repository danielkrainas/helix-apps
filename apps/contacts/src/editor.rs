//! Contact editor form.

use adw::prelude::*;
use gtk::{gio, glib};
use helix_app_common::Labelled;
use helix_eds::vcard::{
    generic_type_index, phone_type_index, Address, Contact, Photo, StructuredName, Typed,
    GENERIC_TYPES, PHONE_TYPES,
};
use std::cell::RefCell;
use std::rc::Rc;

use crate::datefield::DateField;

type Choices = &'static [(&'static str, &'static [&'static str])];

struct TypedRow {
    row: adw::EntryRow,
    types: gtk::DropDown,
    orig_index: u32,
    orig: Typed,
}

/// A group of typed single-line values (emails, phones, websites).
struct TypedList {
    group: adw::PreferencesGroup,
    rows: RefCell<Vec<TypedRow>>,
    choices: Choices,
    /// What one value is: "Email", "Phone Number", "Website".
    kind: &'static str,
    phone: bool,
}

/// Accessible name of a value's Remove button: "Remove Email
/// jane@example.org", or "Remove Email" while the value is empty.
fn remove_name(kind: &str, value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        format!("Remove {kind}")
    } else {
        format!("Remove {kind} {value}")
    }
}

impl TypedList {
    fn new(
        title: &'static str,
        add_label: &'static str,
        choices: Choices,
        phone: bool,
    ) -> Rc<TypedList> {
        let group = adw::PreferencesGroup::builder().title(title).build();
        let list = Rc::new(TypedList {
            group,
            rows: RefCell::default(),
            choices,
            kind: add_label.trim_start_matches("Add "),
            phone,
        });
        let add = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text(add_label)
            .css_classes(["flat"])
            .valign(gtk::Align::Center)
            .build()
            .labelled(helix_app_common::a11y::name_from_tooltip(add_label));
        let weak = Rc::downgrade(&list);
        add.connect_clicked(move |_| {
            if let Some(l) = weak.upgrade() {
                let row = l.add(&Typed::default());
                row.grab_focus();
            }
        });
        list.group.set_header_suffix(Some(&add));
        list
    }

    fn index_for(&self, t: &Typed) -> u32 {
        if t.value.is_empty() && t.types.is_empty() {
            return 0;
        }
        (if self.phone {
            phone_type_index(t)
        } else {
            generic_type_index(&t.types)
        }) as u32
    }

    fn add(self: &Rc<Self>, t: &Typed) -> adw::EntryRow {
        let labels: Vec<&str> = self.choices.iter().map(|(l, _)| *l).collect();
        let types = gtk::DropDown::from_strings(&labels);
        types.set_valign(gtk::Align::Center);
        types.add_css_class("flat");
        helix_app_common::set_accessible_label(&types, &format!("{} Type", self.kind));
        let idx = self.index_for(t);
        types.set_selected(idx);
        let row = adw::EntryRow::builder()
            .title(self.choices[idx as usize].0)
            .text(&t.value)
            .build();
        let r = row.clone();
        let choices = self.choices;
        types.connect_selected_notify(move |d| {
            if let Some((l, _)) = choices.get(d.selected() as usize) {
                r.set_title(l);
            }
        });
        let remove_label = format!("Remove {}", self.kind);
        let remove = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text(&remove_label)
            .css_classes(["flat"])
            .valign(gtk::Align::Center)
            .build()
            .labelled(&remove_name(self.kind, &t.value));
        let (rm, kind) = (remove.clone(), self.kind);
        row.connect_changed(move |e| {
            helix_app_common::set_accessible_label(&rm, &remove_name(kind, &e.text()));
        });
        row.add_suffix(&types);
        row.add_suffix(&remove);
        let weak = Rc::downgrade(self);
        let r = row.clone();
        remove.connect_clicked(move |_| {
            if let Some(l) = weak.upgrade() {
                l.group.remove(&r);
                l.rows.borrow_mut().retain(|x| x.row != r);
            }
        });
        self.group.add(&row);
        self.rows.borrow_mut().push(TypedRow {
            row: row.clone(),
            types,
            orig_index: idx,
            orig: t.clone(),
        });
        row
    }

    fn collect(&self) -> Vec<Typed> {
        self.rows
            .borrow()
            .iter()
            .filter(|r| !r.row.text().trim().is_empty())
            .map(|r| {
                let sel = r.types.selected();
                let types = if sel == r.orig_index && !r.orig.types.is_empty() {
                    r.orig.types.clone()
                } else {
                    self.choices[sel as usize]
                        .1
                        .iter()
                        .map(|s| s.to_string())
                        .collect()
                };
                Typed {
                    types,
                    value: r.row.text().trim().to_string(),
                    pref: r.orig.pref,
                }
            })
            .collect()
    }
}

struct AddressRow {
    group: adw::PreferencesGroup,
    types: gtk::DropDown,
    orig: Address,
    orig_index: u32,
    street: adw::EntryRow,
    extended: adw::EntryRow,
    po_box: adw::EntryRow,
    locality: adw::EntryRow,
    region: adw::EntryRow,
    postal_code: adw::EntryRow,
    country: adw::EntryRow,
}

pub struct Editor {
    pub widget: gtk::ScrolledWindow,
    base: Contact,
    avatar: adw::Avatar,
    photo: RefCell<Option<Photo>>,
    prefix: adw::EntryRow,
    given: adw::EntryRow,
    additional: adw::EntryRow,
    family: adw::EntryRow,
    suffix: adw::EntryRow,
    nickname: adw::EntryRow,
    org: adw::EntryRow,
    org_unit: adw::EntryRow,
    title: adw::EntryRow,
    emails: Rc<TypedList>,
    phones: Rc<TypedList>,
    urls: Rc<TypedList>,
    addresses: RefCell<Vec<AddressRow>>,
    address_box: gtk::Box,
    birthday: Rc<DateField>,
    anniversary: Rc<DateField>,
    note: gtk::TextView,
    /// Address-book chooser for new contacts: (dropdown, book uids).
    pub book_choice: Option<(gtk::DropDown, Vec<String>)>,
    /// What [`Editor::collect`] returned right after the editor was built;
    /// the edit is unsaved while the current result differs.
    initial: RefCell<Option<Result<Contact, String>>>,
}

fn entry(title: &str, text: &str) -> adw::EntryRow {
    adw::EntryRow::builder().title(title).text(text).build()
}

impl Editor {
    /// `books` lists (uid, name) of writable books offered for new contacts;
    /// pass an empty slice when editing an existing contact.
    pub fn new(contact: &Contact, books: &[(String, String)], default_book: &str) -> Rc<Editor> {
        let page = gtk::Box::new(gtk::Orientation::Vertical, 24);
        page.set_margin_top(24);
        page.set_margin_bottom(24);
        page.set_margin_start(12);
        page.set_margin_end(12);

        // Avatar + picture buttons.
        let name = contact.display_name();
        let avatar = adw::Avatar::new(96, Some(&name), true);
        if let Some(bytes) = contact.photo_bytes() {
            set_avatar_bytes(&avatar, &bytes);
        }
        let pic_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        pic_box.set_halign(gtk::Align::Center);
        let change = gtk::Button::with_label("Change Picture…");
        let clear = gtk::Button::with_label("Remove Picture");
        pic_box.append(&change);
        pic_box.append(&clear);
        page.append(&avatar);
        page.append(&pic_box);

        let book_choice = if books.len() > 1 {
            let names: Vec<&str> = books.iter().map(|(_, n)| n.as_str()).collect();
            let dd = gtk::DropDown::from_strings(&names);
            dd.set_valign(gtk::Align::Center);
            let idx = books
                .iter()
                .position(|(u, _)| u == default_book)
                .unwrap_or(0);
            dd.set_selected(idx as u32);
            helix_app_common::set_accessible_label(&dd, "Address Book");
            let row = adw::ActionRow::builder().title("Address Book").build();
            row.add_suffix(&dd);
            let g = adw::PreferencesGroup::new();
            g.add(&row);
            page.append(&g);
            Some((dd, books.iter().map(|(u, _)| u.clone()).collect()))
        } else if books.len() == 1 {
            Some((
                gtk::DropDown::from_strings(&[books[0].1.as_str()]),
                vec![books[0].0.clone()],
            ))
        } else {
            None
        };

        let n = &contact.name;
        let name_group = adw::PreferencesGroup::builder().title("Name").build();
        let prefix = entry("Prefix", &n.prefix);
        let given = entry("First Name", &n.given);
        let additional = entry("Middle Name", &n.additional);
        let family = entry("Last Name", &n.family);
        let suffix = entry("Suffix", &n.suffix);
        let nickname = entry("Nickname", &contact.nickname);
        // A contact with only FN (no N) gets the name split for editing.
        if n.is_empty() && !contact.full_name.is_empty() {
            let guess = StructuredName::guess_from(&contact.full_name);
            given.set_text(&guess.given);
            additional.set_text(&guess.additional);
            family.set_text(&guess.family);
        }
        for r in [&prefix, &given, &additional, &family, &suffix, &nickname] {
            name_group.add(r);
        }
        page.append(&name_group);

        let org_group = adw::PreferencesGroup::builder()
            .title("Organization")
            .build();
        let org = entry("Organization", &contact.org);
        let org_unit = entry("Department", &contact.org_unit);
        let title = entry("Job Title", &contact.title);
        for r in [&org, &org_unit, &title] {
            org_group.add(r);
        }
        page.append(&org_group);

        let emails = TypedList::new("Email Addresses", "Add Email", GENERIC_TYPES, false);
        for e in &contact.emails {
            emails.add(e);
        }
        let phones = TypedList::new("Phone Numbers", "Add Phone Number", PHONE_TYPES, true);
        for p in &contact.phones {
            phones.add(p);
        }
        let urls = TypedList::new("Websites", "Add Website", GENERIC_TYPES, false);
        for u in &contact.urls {
            urls.add(u);
        }
        page.append(&emails.group);
        page.append(&phones.group);

        let address_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let addr_header = adw::PreferencesGroup::builder().title("Addresses").build();
        let add_addr = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("Add Address")
            .css_classes(["flat"])
            .valign(gtk::Align::Center)
            .build()
            .labelled("Add Address");
        addr_header.set_header_suffix(Some(&add_addr));
        page.append(&addr_header);
        page.append(&address_box);
        page.append(&urls.group);

        let misc = adw::PreferencesGroup::builder().title("Other").build();
        let birthday = DateField::new("Birthday", &contact.birthday);
        let anniversary = DateField::new("Anniversary", &contact.anniversary);
        misc.add(&birthday.row);
        misc.add(&anniversary.row);
        page.append(&misc);

        let notes_group = adw::PreferencesGroup::builder().title("Notes").build();
        let note = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .top_margin(8)
            .bottom_margin(8)
            .left_margin(8)
            .right_margin(8)
            .height_request(96)
            .build();
        helix_app_common::set_accessible_label(&note, "Notes");
        note.buffer().set_text(&contact.note);
        let frame = gtk::Frame::builder()
            .child(&note)
            .css_classes(["view"])
            .build();
        notes_group.add(&frame);
        page.append(&notes_group);

        let clamp = adw::Clamp::builder().maximum_size(600).child(&page).build();
        let widget = gtk::ScrolledWindow::builder()
            .child(&clamp)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();

        let ed = Rc::new(Editor {
            widget,
            base: contact.clone(),
            avatar,
            photo: RefCell::new(contact.photo.clone()),
            prefix,
            given,
            additional,
            family,
            suffix,
            nickname,
            org,
            org_unit,
            title,
            emails,
            phones,
            urls,
            addresses: RefCell::default(),
            address_box,
            birthday,
            anniversary,
            note,
            book_choice,
            initial: RefCell::default(),
        });
        for a in &contact.addresses {
            ed.add_address(a);
        }
        *ed.initial.borrow_mut() = Some(ed.collect());
        let weak = Rc::downgrade(&ed);
        add_addr.connect_clicked(move |_| {
            if let Some(e) = weak.upgrade() {
                e.add_address(&Address::default());
            }
        });
        let weak = Rc::downgrade(&ed);
        clear.connect_clicked(move |_| {
            if let Some(e) = weak.upgrade() {
                *e.photo.borrow_mut() = None;
                e.avatar.set_custom_image(None::<&gtk::gdk::Paintable>);
            }
        });
        let weak = Rc::downgrade(&ed);
        change.connect_clicked(move |b| {
            let Some(e) = weak.upgrade() else { return };
            let filter = gtk::FileFilter::new();
            filter.set_name(Some("Images"));
            filter.add_mime_type("image/*");
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            let dialog = gtk::FileDialog::builder()
                .title("Choose a Picture")
                .filters(&filters)
                .build();
            let win = b.root().and_downcast::<gtk::Window>();
            let weak = Rc::downgrade(&e);
            dialog.open(win.as_ref(), None::<&gio::Cancellable>, move |res| {
                let (Some(e), Ok(file)) = (weak.upgrade(), res) else {
                    return;
                };
                if let Some(path) = file.path() {
                    if let Ok(bytes) = std::fs::read(&path) {
                        if set_avatar_bytes(&e.avatar, &bytes) {
                            let mime = mime_for(&path);
                            *e.photo.borrow_mut() = Some(Photo::Inline { mime, data: bytes });
                        }
                    }
                }
            });
        });
        ed
    }

    fn add_address(self: &Rc<Self>, a: &Address) {
        let labels: Vec<&str> = GENERIC_TYPES.iter().map(|(l, _)| *l).collect();
        let types = gtk::DropDown::from_strings(&labels);
        types.set_valign(gtk::Align::Center);
        helix_app_common::set_accessible_label(&types, "Address Type");
        let idx = if a.is_empty() && a.types.is_empty() {
            0
        } else {
            generic_type_index(&a.types) as u32
        };
        types.set_selected(idx);
        let remove = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text("Remove Address")
            .css_classes(["flat"])
            .valign(gtk::Align::Center)
            .build()
            .labelled(&remove_name(
                "Address",
                &address_summary(&a.street, &a.locality),
            ));
        let suffix = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        suffix.append(&types);
        suffix.append(&remove);
        let group = adw::PreferencesGroup::new();
        group.set_header_suffix(Some(&suffix));
        let row = AddressRow {
            group: group.clone(),
            types,
            orig: a.clone(),
            orig_index: idx,
            street: entry("Street", &a.street),
            extended: entry("Extended Address", &a.extended),
            po_box: entry("PO Box", &a.po_box),
            locality: entry("City", &a.locality),
            region: entry("State/Province", &a.region),
            postal_code: entry("Postal Code", &a.postal_code),
            country: entry("Country", &a.country),
        };
        for e in [
            &row.street,
            &row.extended,
            &row.po_box,
            &row.locality,
            &row.region,
            &row.postal_code,
            &row.country,
        ] {
            group.add(e);
        }
        self.address_box.append(&group);
        // The Remove button names the address by its street and city.
        for e in [&row.street, &row.locality] {
            let (rm, street, city) = (remove.clone(), row.street.clone(), row.locality.clone());
            e.connect_changed(move |_| {
                let summary = address_summary(&street.text(), &city.text());
                helix_app_common::set_accessible_label(&rm, &remove_name("Address", &summary));
            });
        }
        let weak = Rc::downgrade(self);
        let g = group.clone();
        remove.connect_clicked(move |_| {
            if let Some(e) = weak.upgrade() {
                e.address_box.remove(&g);
                e.addresses.borrow_mut().retain(|r| r.group != g);
            }
        });
        self.addresses.borrow_mut().push(row);
    }

    /// The edited contact, or an error message.
    pub fn collect(&self) -> Result<Contact, String> {
        let mut c = self.base.clone();
        let t = |e: &adw::EntryRow| e.text().trim().to_string();
        c.name = StructuredName {
            prefix: t(&self.prefix),
            given: t(&self.given),
            additional: t(&self.additional),
            family: t(&self.family),
            suffix: t(&self.suffix),
        };
        c.full_name = c.name.display();
        c.file_as.clear();
        c.nickname = t(&self.nickname);
        c.org = t(&self.org);
        c.org_unit = t(&self.org_unit);
        c.title = t(&self.title);
        c.emails = self.emails.collect();
        c.phones = self.phones.collect();
        c.urls = self.urls.collect();
        c.addresses = self
            .addresses
            .borrow()
            .iter()
            .map(|r| {
                let sel = r.types.selected();
                let types = if sel == r.orig_index && !r.orig.types.is_empty() {
                    r.orig.types.clone()
                } else {
                    GENERIC_TYPES[sel as usize]
                        .1
                        .iter()
                        .map(|s| s.to_string())
                        .collect()
                };
                Address {
                    types,
                    po_box: t(&r.po_box),
                    extended: t(&r.extended),
                    street: t(&r.street),
                    locality: t(&r.locality),
                    region: t(&r.region),
                    postal_code: t(&r.postal_code),
                    country: t(&r.country),
                }
            })
            .filter(|a| !a.is_empty())
            .collect();
        c.birthday = self.birthday.value()?;
        c.anniversary = self.anniversary.value()?;
        let buf = self.note.buffer();
        c.note = buf
            .text(&buf.start_iter(), &buf.end_iter(), false)
            .trim_end()
            .to_string();
        c.photo = self.photo.borrow().clone();
        let empty = c.display_name().is_empty()
            && c.note.is_empty()
            && c.addresses.is_empty()
            && c.urls.is_empty();
        if empty {
            return Err("Enter a name or some details first".into());
        }
        Ok(c)
    }

    /// Whether the fields differ from what the editor started with.
    pub fn is_dirty(&self) -> bool {
        has_changes(self.initial.borrow().as_ref(), &self.collect())
    }

    pub fn chosen_book(&self) -> Option<String> {
        self.book_choice
            .as_ref()
            .and_then(|(dd, uids)| uids.get(dd.selected() as usize).cloned())
    }

    /// Focuses the first invalid date field, if any (after a failed save).
    pub fn focus_invalid(&self) -> bool {
        for f in [&self.birthday, &self.anniversary] {
            if f.value().is_err() {
                f.focus();
                return true;
            }
        }
        false
    }

    pub fn focus_first(&self) {
        self.given.grab_focus();
    }
}

/// Whether `now` differs from the snapshot taken when editing started.
/// Comparing the collected contacts (rather than tracking edits) means that
/// typing and then undoing a change leaves the editor clean.
fn has_changes(initial: Option<&Result<Contact, String>>, now: &Result<Contact, String>) -> bool {
    initial != Some(now)
}

/// Loads image bytes into an avatar; returns false if they are not an image.
pub fn set_avatar_bytes(avatar: &adw::Avatar, bytes: &[u8]) -> bool {
    match gtk::gdk::Texture::from_bytes(&glib::Bytes::from(bytes)) {
        Ok(t) => {
            avatar.set_custom_image(Some(&t));
            true
        }
        Err(_) => false,
    }
}

fn mime_for(path: &std::path::Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(
        match ext.as_str() {
            "jpg" | "jpeg" => "image/jpeg",
            "png" => "image/png",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "svg" => "image/svg+xml",
            _ => return None,
        }
        .to_string(),
    )
}

/// An address in a few words for a button name: "1 Main St, Springfield".
fn address_summary(street: &str, city: &str) -> String {
    [street.trim(), city.trim()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_buttons_name_their_value() {
        assert_eq!(
            remove_name("Email", "jane@example.org"),
            "Remove Email jane@example.org"
        );
        assert_eq!(remove_name("Phone Number", "  "), "Remove Phone Number");
        assert_eq!(
            remove_name("Address", &address_summary("1 Main St", "Springfield")),
            "Remove Address 1 Main St, Springfield"
        );
        assert_eq!(address_summary("", "Springfield"), "Springfield");
    }

    #[test]
    fn unchanged_edit_is_clean() {
        let c = Contact {
            nickname: "Bo".into(),
            ..Contact::default()
        };
        let start: Result<Contact, String> = Ok(c.clone());
        assert!(!has_changes(Some(&start), &Ok(c.clone())));
        let edited = Contact {
            nickname: "Bob".into(),
            ..c
        };
        assert!(has_changes(Some(&start), &Ok(edited)));
        // An invalid field is a change too (the user typed something).
        assert!(has_changes(Some(&start), &Err("bad birthday".into())));
        // An empty new contact stays clean until something is entered.
        let empty: Result<Contact, String> = Err("empty".into());
        assert!(!has_changes(Some(&empty), &Err("empty".into())));
    }
}
