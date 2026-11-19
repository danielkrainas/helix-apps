# Contacts

Drop-in replacement for GNOME Contacts.

- **App ID**: `org.helixdesktop.Contacts`
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
    `gtk::UriLauncher` (OpenURI portal when sandboxed). Each such row
    shows a labelled button for it (*Send Email*, *Call*, *Show on Map*,
    *Open Website*; also the row's accessible description), and
    activating the row does the same. When no application handles the
    link a toast says so ("No app available to send email").
  - Each value has a copy button, named with its value for assistive
    technology ("Copy jane@example.org").
- **Empty states**: while the address books load, the list shows a
  spinner named "Loading contacts" with "Loading Contacts…" below it; no
  contacts → status page with *Create Contact* and *Import…* (the content
  pane stays blank, not "Select a Contact", both then and while loading);
  no selection → "Select a contact"; no search results → "No Results".
- Type-to-search: typing while focus is anywhere in the sidebar (for
  example on the contact list) goes to the search entry.
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
  a remove button, plus *Add* buttons; birthday and anniversary (each a
  day drop-down, a month drop-down, an optional year field and a
  calendar popover — no ISO typing; stored as `YYYY-MM-DD`, or `--MM-DD`
  without a year, and an unchanged date keeps its original text); notes
  (multi-line). An impossible or incomplete date ("April has only 30
  days", "Choose both a day and a month") is shown under the date and
  marks the control (invalid state, error-message relation, announced);
  *Done* then moves focus there.
- Leaving the editor any other way than *Done* / *Cancel* — `Escape`
  (also while typing in a field), selecting another contact, or *New
  Contact* (`Ctrl+N`) — first checks for unsaved changes. Without changes
  the editor simply closes; with changes a "Discard Changes?" alert asks
  first (*Keep Editing* is the default and the Escape response, *Discard*
  is destructive). Changes are detected by comparing the editor's current
  contents with what it started with.
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
  non-empty name/org/etc., all distinct emails/phones/addresses/urls). In
  the list a link icon marks them, and the row's accessible description
  says "Linked contact".
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
- The row keeps keyboard focus: Space / Enter (or a click anywhere on the
  row) toggles its check box in place, without rebuilding the list, so
  focus and scroll position stay put. The check box is not separately
  focusable or clickable but is never insensitive; it is named "Select
  NAME", and the row exposes the checked / not checked state to
  assistive technology.
- The sidebar title shows the count: "Select Contacts" with nothing
  chosen, then "1 Selected", "N Selected"; it returns to "Contacts" on
  leaving selection mode.

### Import / export
- *Import…* (primary menu) opens a `.vcf` file (one or many vCards, versions
  2.1/3.0/4.0) and creates them in the default address book. Only inline
  photos (`ENCODING=b` or `data:` URIs) of at most 10 MiB are imported;
  linked `PHOTO` URIs are dropped, since a `file://` URI could name any
  local file.
- *Export…* (selection mode or contact menu) saves the chosen contacts as a
  single `.vcf` (vCard 3.0). Photos stored by EDS as files are embedded
  inline; other `file://` photo URIs are left out.

### Photos
- A photo is shown when it is inline, or a `file://` URI naming a regular
  file directly inside an EDS `photos` directory
  (`$XDG_DATA_HOME/evolution/addressbook/<book>/photos/` for local books,
  the same under `$XDG_CACHE_HOME` for cached remote books), after
  resolving symlinks and `..`. EDS stores inline photos there itself, but
  keeps any other URI as given, so a contact from a shared or synced book
  may name an arbitrary path: such photos are never read. Photos over
  10 MiB are not shown.

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
quit, `Ctrl+W` close window, `Ctrl+?` shortcuts. `Ctrl+?` (and the menu
item) opens a `GtkShortcutsWindow` built from a table
(`helix_app_common::shortcuts`); a unit test checks it lists every
registered accelerator.

### Command line
- `helix-contacts` opens the window.
- `--individual UID` (`-i`) selects that contact on start.
- `--email ADDRESS` (`-e`) selects the contact with that address, or opens
  the new-contact editor pre-filled with it.
- `--search TERM` (`-s`) starts with the search filled in.
- `FILE.vcf` arguments are imported into the default address book once
  the address books have loaded.

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

### Accessibility
- Status changes that happen without focus moving are announced to
  screen readers (`helix_app_common::a11y`): the number of contacts matching a search once typing pauses; "Loading contacts" when the list starts loading and "N contacts loaded" / "No contacts" when it finishes.
- Focus is kept across list rebuilds (`helix_app_common::focus`): Rebuilding the contact list (entering / leaving selection mode, search, address-book changes) keeps keyboard focus on the same contact and the scroll position.
- Editor controls have specific names: each value's Remove button names
  the value ("Remove Email jane@example.org", "Remove Address 1 Main St,
  Springfield"), following edits; type drop-downs are "Email Type",
  "Phone Number Type", "Website Type", "Address Type"; the address-book
  drop-downs are "Address Book" (editor) and "Address Book for New
  Contacts" (first-run dialog); the notes field is "Notes"; the list's
  search entry is "Search Contacts".
