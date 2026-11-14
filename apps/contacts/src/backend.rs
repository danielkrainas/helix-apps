//! Worker thread owning the EDS connection, the opened address books and
//! their live views. The UI sends [`Cmd`]s and receives [`Msg`]s.

use helix_eds::vcard::Contact;
use helix_eds::{sexp, Book, Eds, Source, SourceKind, View, ViewEvent, Watch};
use std::collections::HashMap;
use std::sync::mpsc;

#[derive(Debug)]
pub enum Cmd {
    Create {
        book: String,
        contact: Box<Contact>,
        tag: u64,
    },
    Modify {
        book: String,
        contact: Box<Contact>,
    },
    Remove {
        book: String,
        uids: Vec<String>,
    },
    Import {
        book: String,
        contacts: Vec<Contact>,
    },
    /// Reconnect (after failure) or re-read the source list.
    Resync,
}

#[derive(Debug)]
pub enum Msg {
    Unavailable(String),
    Sources(Vec<Source>),
    BookOpened {
        uid: String,
        writable: bool,
    },
    BookClosed(String),
    Added {
        book: String,
        contacts: Vec<Contact>,
    },
    Modified {
        book: String,
        contacts: Vec<Contact>,
    },
    Removed {
        book: String,
        uids: Vec<String>,
    },
    Loaded(String),
    Created {
        book: String,
        uids: Vec<String>,
        tag: u64,
    },
    Imported(usize),
    Error(String),
}

#[derive(Clone)]
pub struct Backend {
    tx: mpsc::Sender<Cmd>,
}

impl Backend {
    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
}

struct OpenBook {
    // Field order matters: the view is disposed before the book closes.
    _view: View,
    book: Book,
}

pub fn start(out: async_channel::Sender<Msg>) -> Backend {
    let (tx, rx) = mpsc::channel::<Cmd>();
    let resync_tx = tx.clone();
    std::thread::Builder::new()
        .name("contacts-eds".into())
        .spawn(move || worker(rx, resync_tx, out))
        .expect("spawn EDS worker");
    Backend { tx }
}

fn worker(rx: mpsc::Receiver<Cmd>, self_tx: mpsc::Sender<Cmd>, out: async_channel::Sender<Msg>) {
    let mut eds: Option<Eds> = None;
    let mut _watch: Option<Watch> = None;
    let mut books: HashMap<String, OpenBook> = HashMap::new();
    let send = |m: Msg| {
        let _ = out.send_blocking(m);
    };
    let mut pending = Some(Cmd::Resync);
    loop {
        let cmd = match pending.take() {
            Some(c) => c,
            None => match rx.recv() {
                Ok(c) => c,
                Err(_) => return,
            },
        };
        if eds.is_none() {
            match Eds::session().and_then(|e| e.sources().map(|_| e)) {
                Ok(e) => {
                    let t = self_tx.clone();
                    _watch = e
                        .watch_sources(move || {
                            let _ = t.send(Cmd::Resync);
                        })
                        .ok();
                    eds = Some(e);
                }
                Err(e) => {
                    send(Msg::Unavailable(e.to_string()));
                    continue;
                }
            }
        }
        let e = eds.as_ref().unwrap();
        match cmd {
            Cmd::Resync => {
                // Coalesce bursts of registry signals.
                while let Ok(next) = rx.try_recv() {
                    if !matches!(next, Cmd::Resync) {
                        pending = Some(next);
                        break;
                    }
                }
                sync(e, &mut books, &out);
            }
            Cmd::Create { book, contact, tag } => match books.get(&book) {
                Some(b) => match b.book.create(&[*contact]) {
                    Ok(uids) => send(Msg::Created { book, uids, tag }),
                    Err(err) => send(Msg::Error(format!("Could not create contact: {err}"))),
                },
                None => send(Msg::Error("The address book is not available".into())),
            },
            Cmd::Modify { book, contact } => {
                if let Some(b) = books.get(&book) {
                    if let Err(err) = b.book.modify(&[*contact]) {
                        send(Msg::Error(format!("Could not save contact: {err}")));
                    }
                }
            }
            Cmd::Remove { book, uids } => {
                if let Some(b) = books.get(&book) {
                    if let Err(err) = b.book.remove(&uids) {
                        send(Msg::Error(format!("Could not delete contact: {err}")));
                    }
                }
            }
            Cmd::Import { book, contacts } => match books.get(&book) {
                Some(b) => {
                    let n = contacts.len();
                    let contacts: Vec<Contact> = contacts
                        .into_iter()
                        .map(|mut c| {
                            c.uid.clear();
                            c.rev.clear();
                            c
                        })
                        .collect();
                    match b.book.create(&contacts) {
                        Ok(_) => send(Msg::Imported(n)),
                        Err(err) => send(Msg::Error(format!("Import failed: {err}"))),
                    }
                }
                None => send(Msg::Error("The address book is not available".into())),
            },
        }
    }
}

fn sync(eds: &Eds, books: &mut HashMap<String, OpenBook>, out: &async_channel::Sender<Msg>) {
    let send = |m: Msg| {
        let _ = out.send_blocking(m);
    };
    let sources = match eds.sources() {
        Ok(s) => s,
        Err(e) => {
            send(Msg::Error(e.to_string()));
            return;
        }
    };
    let wanted: Vec<&Source> = sources
        .iter()
        .filter(|s| s.is(SourceKind::AddressBook) && s.enabled())
        .collect();
    // Parent collections can be disabled as a whole (e.g. a GOA account).
    let parent_enabled = |s: &Source| {
        s.parent()
            .and_then(|p| sources.iter().find(|x| x.uid == p))
            .map(|p| p.enabled())
            .unwrap_or(true)
    };
    let wanted: Vec<&Source> = wanted.into_iter().filter(|s| parent_enabled(s)).collect();
    send(Msg::Sources(sources.clone()));
    let stale: Vec<String> = books
        .keys()
        .filter(|k| !wanted.iter().any(|s| &s.uid == *k))
        .cloned()
        .collect();
    for uid in stale {
        books.remove(&uid);
        send(Msg::BookClosed(uid));
    }
    for s in wanted {
        if books.contains_key(&s.uid) {
            continue;
        }
        let book = match eds.open_book(&s.uid) {
            Ok(b) => b,
            Err(e) => {
                send(Msg::Error(format!(
                    "Could not open “{}”: {e}",
                    s.display_name()
                )));
                continue;
            }
        };
        send(Msg::BookOpened {
            uid: s.uid.clone(),
            writable: book.writable,
        });
        let tx = out.clone();
        let uid = s.uid.clone();
        let view = book.view(&sexp::all_contacts(), move |ev| {
            let parse = |v: Vec<String>| {
                v.iter()
                    .filter_map(|c| Contact::parse(c))
                    .collect::<Vec<_>>()
            };
            let m = match ev {
                ViewEvent::Added(v) => Msg::Added {
                    book: uid.clone(),
                    contacts: parse(v),
                },
                ViewEvent::Modified(v) => Msg::Modified {
                    book: uid.clone(),
                    contacts: parse(v),
                },
                ViewEvent::Removed(v) => Msg::Removed {
                    book: uid.clone(),
                    uids: v,
                },
                ViewEvent::Complete(_) => Msg::Loaded(uid.clone()),
                ViewEvent::Progress(..) => return,
            };
            let _ = tx.send_blocking(m);
        });
        match view {
            Ok(v) => {
                books.insert(s.uid.clone(), OpenBook { _view: v, book });
            }
            Err(e) => send(Msg::Error(format!(
                "Could not read “{}”: {e}",
                s.display_name()
            ))),
        }
    }
}
