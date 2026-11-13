# Contacts

Drop-in replacement for GNOME Contacts.

- **App ID**: `org.helix.Contacts`
- **Binary**: `helix-contacts`
- **Crate**: `apps/contacts`

## Purpose
Manage contacts.

## Main windows
- Contact list and details.
- Editor.
- Linking / merging.

## Contracts used
- Evolution Data Server or equivalent.
- GNOME Online Accounts.
- XDG portals: OpenURI.
- GSettings: `org.gnome.Contacts`.

## Non-goals
- Full EDS backend parity on day one.

---

## Behaviour (derived spec)

### Data source
- Contacts live in **Evolution Data Server**. Helix has no EDS client
  libraries, so the shared crate `crates/helix-eds` talks to EDS over D-Bus
  (zbus): the source registry (`org.gnome.evolution.dataserver.Sources5`),
  the address-book factory (`…AddressBook10`, `OpenAddressBook`) and the
  per-book `AddressBook` / `AddressBookView` objects. vCards are parsed and
  written by `helix-eds` in pure Rust.
- Every **enabled** source with an `[Address Book]` extension is opened:
  the local "Personal" book, plus whatever GNOME Online Accounts (Google,
  Nextcloud/CardDAV, Exchange…) has configured in EDS. GOA itself is never
  called directly; accounts appear through EDS.
- Each book gets a live view (`GetView` + `Start`, notify-initial). Added /
  modified / removed contacts update the list immediately, including
  changes made by other apps. Registry changes (accounts added/removed,
  books enabled/disabled) re-sync the set of open books.
- If EDS is not reachable the window shows a status page explaining that
  the contacts service is unavailable, with a *Retry* button.
- **Later phase (not built)**: a Helix-native local vCard store with direct
  CardDAV sync, for systems without EDS. The UI talks to a small backend
  module so another store can be slotted in.

### Window
- `AdwApplicationWindow` with an `AdwNavigationSplitView`.
- **Sidebar**: header bar with *New Contact* (`+`), *Select* toggle and the
  primary menu; a search entry; the contact list. Rows show an `AdwAvatar`
  (photo or initials) and the display name. Rows are sorted by first name
  or surname (`sort-on-surname`) and grouped under initial-letter headers
  (`#` for non-letters). The user's own contact ("Me", from
  `org.gnome.evolution-data-server.addressbook self-contact-uid`) is listed
  first with a "Me" marker.
- **Content**: the selected contact. Large avatar, name, nickname and
  organisation/title, then groups of rows: email addresses, phone numbers,
  postal addresses, websites, birthday / anniversary, notes, and the
  address book(s) the contact comes from. Each row shows the value with its
  type ("Work", "Mobile", …).
  - Email rows open `mailto:`; phone rows open `tel:`; addresses open
    `geo:0,0?q=<address>` (maps); websites open the URL — all through
    `gtk::UriLauncher` (OpenURI portal when sandboxed).
  - Each value has a copy button.
- **Empty states**: no contacts → status page with *Create Contact* and
  *Import…*; no selection → "Select a contact"; no search results →
  "No Results".
- On narrow windows the split view collapses (sidebar first, content
  pushed on selection), via an `AdwBreakpoint` at 500sp.

### Editing
- *Edit* (or `Ctrl+E`) turns the content page into the editor:
  name fields (prefix, first, middle, last, suffix), nickname, avatar
  (*Change picture…* file chooser / *Remove picture*), organisation,
  department, job title, then repeatable rows for emails, phones,
  addresses (street, extended, PO box, city, region, postal code,
  country), websites — each with a type dropdown (phones: Mobile, Home,
  Work, Home Fax, Work Fax, Pager, Other; others: Personal, Work, Other) and
  a remove button, plus *Add* buttons; birthday (date entry `YYYY-MM-DD`
  or `--MM-DD` with a calendar popover); notes (multi-line).
- *Done* saves (`ModifyContacts`), *Cancel* discards. Unknown vCard
  properties (custom fields, other apps' extensions) are preserved.
- `X-EVOLUTION-FILE-AS` is filled ("Last, First") because EDS requires it.
- Read-only books (e.g. LDAP, some online accounts) hide *Edit*.
- *New Contact* opens an empty editor; the target address book is the
  default one (`org.gnome.Evolution.DefaultSources default-address-book`,
  falling back to `system-address-book`) and can be changed from a
  dropdown in the new-contact editor when more than one writable book
  exists. Saving with every field empty is refused with a toast.

### Deleting
- *Delete* (content menu, `Delete` key, or selection mode) removes the
  contact from the list immediately and shows an "N contacts deleted"
  toast with *Undo*. The EDS removal happens when the toast is dismissed
  without undo (or when the window closes).

### Linking (merging)
- EDS has no persona-linking store (GNOME uses libfolks'), so Helix keeps
  link groups in `$XDG_DATA_HOME/helix-contacts/links.json` (list of
  `[book-uid, contact-uid]` groups). Linking is non-destructive.
- In selection mode, selecting 2+ contacts enables *Link*. Linked contacts
  show as one entry whose details are the union of all members (first
  non-empty name/org/etc., all distinct emails/phones/addresses/urls).
- The content menu of a linked contact offers *Unlink*, which splits the
  group back into separate contacts.
- Editing a linked contact edits its first writable member.
- Members whose contact disappears are dropped from groups on the next
  save.

#### Links made in GNOME Contacts (read-only import)
GNOME Contacts links through libfolks. Helix reads folks' link data and
merges it with `links.json` every time the list is rebuilt; it never
writes folks' data.
- **EDS contact attributes** (the normal case — folks' primary store is
  EDS `system-address-book`): a contact carrying `X-FOLKS-CONTACTS-IDS`
  is linked with every contact named there. Values are folks local IDs,
  for EDS personas the IID `<source-uid>:<contact-uid>`; a full persona UID
  `eds:<source-uid>:<contact-uid>` (`\:` escapes) is accepted too. Values
  may be repeated attributes or `,`/`;` lists. `X-FOLKS-ANTI-LINKS`
  (persona UIDs) on the same contact excludes those contacts.
- **Key-file store** `$XDG_DATA_HOME/folks/relationships.ini` (only used
  when the folks key-file backend was made primary): a GKeyFile whose
  groups are personas and whose keys are IM protocols / web services with
  `;`-separated addresses (`__`-prefixed keys are metadata and skipped).
  EDS contacts sharing one of a persona's addresses (IM address from
  `IMPP`/`X-JABBER`/`X-AIM`/…, or their IID) are linked, as folks does.
  Read once at startup.
- *Unlink* on an individual that includes folks-derived links records its
  members in `links.json` (`ignore_external`), so the folks link is
  ignored by Helix from then on (GNOME Contacts still sees it).
- Not verified against real folks data (none exists on the dev machine):
  the exact IID format and list separators follow the public folks
  documentation / general knowledge; links to non-EDS personas
  (Telepathy, etc.) are ignored.

### Selection mode
- Toggled with the *Select* button. Rows get check buttons; an action bar
  offers *Link*, *Export…* and *Delete*. `Escape` leaves selection mode.

### Import / export
- *Import…* (primary menu) opens a `.vcf` file (one or many vCards, versions
  2.1/3.0/4.0) and creates them in the default address book.
- *Export…* (selection mode or contact menu) saves the chosen contacts as a
  single `.vcf` (vCard 3.0).

### Preferences / menu
- Primary menu: *Sort by First Name / Surname* (radio), *Import…*,
  *Address Book* submenu choosing the default book for new contacts,
  *Keyboard Shortcuts*, *About Contacts*.

### Initial setup
- When `did-initial-setup` is false and more than one writable address
  book exists, the first launch shows a page asking which address book to
  use for new contacts (local "Personal" pre-selected); confirming sets
  the default and `did-initial-setup` to true. With only one book, the
  key is simply set.

### GSettings
- `org.gnome.Contacts` (GNOME Contacts' schema; shipped in
  `data/org.gnome.Contacts.gschema.xml` with only these keys, copied
  verbatim from the real schema):
  `did-initial-setup` (b, false), `sort-on-surname` (b, false),
  `window-width` (i, 800), `window-height` (i, 600),
  `window-maximized` (b, false). The real schema's `window-fullscreen`
  is not used (no fullscreen mode).
- `org.gnome.Evolution.DefaultSources` (installed by EDS):
  `default-address-book` (s) — read and written.
- `org.gnome.evolution-data-server.addressbook` (installed by EDS):
  `self-contact-uid` (s) — read only.
- Read through `helix_app_common::Prefs`, so a missing schema falls back to
  in-memory defaults.

### Keyboard shortcuts
`Ctrl+N` new contact, `Ctrl+F` search, `Ctrl+E` edit, `Ctrl+I` import,
`Delete` delete
contact, `Escape` cancel edit / leave selection / clear search, `Ctrl+Q`
quit, `Ctrl+W` close window, `Ctrl+?` shortcuts.

### Command line
- `helix-contacts` opens the window.
- `--individual UID` (`-i`) selects that contact on start.
- `--email ADDRESS` (`-e`) selects the contact with that address, or opens
  the new-contact editor pre-filled with it.
- `--search TERM` (`-s`) starts with the search filled in.
- `FILE.vcf` arguments are imported.

### Testing
- `crates/helix-eds` unit tests cover vCard parsing/serialization.
- `crates/helix-eds/tests/private_bus.rs` exercises create / modify /
  remove / views against real EDS daemons on a **private** D-Bus
  (`tests/run-private-eds.sh`), never the user's session bus.
- App unit tests cover linking, merging, grouping and sorting, and the
  folks link parsers (`src/folks.rs`).

## Decisions
- Backend: talk to Evolution Data Server over D-Bus (zbus), sharing the
  client in `crates/helix-eds` with Calendar; a local vCard store with
  CardDAV sync is a later phase.
- GSettings: aligned with the real `org.gnome.Contacts` schema — all five
  keys used already matched in name/type/default; no invented keys.
- GNOME links: read folks' link data (EDS `X-FOLKS-*` attributes and
  `relationships.ini`) read-only and merge it with Helix's `links.json`;
  Helix-made links stay in `links.json` only.
