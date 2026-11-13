//! Minimal Secret Service (`org.freedesktop.secrets`) client over zbus.
//!
//! Only what the password manager needs: open a session, search, read,
//! create/replace and delete items in the default collection, and follow
//! unlock/create prompts. Every call is blocking; callers run it on a worker
//! thread. Items use the Helix schema documented in SPEC.md:
//!
//! | attribute | value |
//! |---|---|
//! | `xdg:schema` | `org.helix.Web.Password` |
//! | `origin` | `scheme://host[:port]` of the page with the login form |
//! | `target-origin` | origin the form submits to |
//! | `username` | the user name (may be empty) |

use std::collections::HashMap;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

pub const SCHEMA: &str = "org.helix.Web.Password";
pub const ATTR_SCHEMA: &str = "xdg:schema";
pub const ATTR_ORIGIN: &str = "origin";
pub const ATTR_TARGET: &str = "target-origin";
pub const ATTR_USERNAME: &str = "username";

const BUS: &str = "org.freedesktop.secrets";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const SERVICE: &str = "org.freedesktop.Secret.Service";
const COLLECTION: &str = "org.freedesktop.Secret.Collection";
const ITEM: &str = "org.freedesktop.Secret.Item";
const PROMPT: &str = "org.freedesktop.Secret.Prompt";
const CONTENT_TYPE: &str = "text/plain; charset=utf8";

/// A stored site login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Login {
    pub origin: String,
    pub target_origin: String,
    pub username: String,
    pub password: String,
}

impl Login {
    pub fn label(&self) -> String {
        let host = self
            .origin
            .split_once("://")
            .map(|(_, h)| h)
            .unwrap_or(&self.origin);
        if self.username.is_empty() {
            format!("Password for {host}")
        } else {
            format!("Password for {} on {host}", self.username)
        }
    }

    pub fn attributes(&self) -> HashMap<String, String> {
        HashMap::from([
            (ATTR_SCHEMA.to_string(), SCHEMA.to_string()),
            (ATTR_ORIGIN.to_string(), self.origin.clone()),
            (ATTR_TARGET.to_string(), self.target_origin.clone()),
            (ATTR_USERNAME.to_string(), self.username.clone()),
        ])
    }
}

/// Search attributes for an origin (and optionally a user name).
pub fn query(origin: &str, username: Option<&str>) -> HashMap<String, String> {
    let mut q = HashMap::from([
        (ATTR_SCHEMA.to_string(), SCHEMA.to_string()),
        (ATTR_ORIGIN.to_string(), origin.to_string()),
    ]);
    if let Some(u) = username {
        q.insert(ATTR_USERNAME.to_string(), u.to_string());
    }
    q
}

#[derive(Debug)]
pub enum Error {
    Bus(zbus::Error),
    /// The user dismissed an unlock/confirmation prompt.
    Dismissed,
    Unavailable,
}

impl From<zbus::Error> for Error {
    fn from(e: zbus::Error) -> Self {
        Error::Bus(e)
    }
}

impl From<zbus::fdo::Error> for Error {
    fn from(e: zbus::fdo::Error) -> Self {
        Error::Bus(e.into())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Bus(e) => write!(f, "Secret Service error: {e}"),
            Error::Dismissed => write!(f, "the keyring prompt was dismissed"),
            Error::Unavailable => write!(f, "no Secret Service collection is available"),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

pub struct Client {
    conn: Connection,
    session: OwnedObjectPath,
}

fn is_none_path(p: &ObjectPath<'_>) -> bool {
    p.as_str() == "/"
}

impl Client {
    /// Connects to the Secret Service on the user's session bus.
    pub fn session_bus() -> Result<Client> {
        Client::new(Connection::session()?)
    }

    /// Opens a "plain" transfer session on `conn` (the session bus, or a
    /// private connection in tests).
    pub fn new(conn: Connection) -> Result<Client> {
        let svc = Proxy::new(&conn, BUS, SERVICE_PATH, SERVICE)?;
        let (_out, session): (OwnedValue, OwnedObjectPath) =
            svc.call("OpenSession", &("plain", Value::from("")))?;
        Ok(Client { conn, session })
    }

    fn proxy<'a>(&'a self, path: &'a str, iface: &'a str) -> Result<Proxy<'a>> {
        Ok(Proxy::new(&self.conn, BUS, path, iface)?)
    }

    /// Runs a prompt and waits for its `Completed` signal.
    fn prompt(&self, path: &ObjectPath<'_>) -> Result<()> {
        if is_none_path(path) {
            return Ok(());
        }
        let p = self.proxy(path.as_str(), PROMPT)?;
        let mut completed = p.receive_signal("Completed")?;
        let () = p.call("Prompt", &(""))?;
        let msg = completed.next().ok_or(Error::Unavailable)?;
        let (dismissed, _result): (bool, OwnedValue) = msg.body().deserialize()?;
        if dismissed {
            Err(Error::Dismissed)
        } else {
            Ok(())
        }
    }

    fn search_paths(&self, attrs: &HashMap<String, String>) -> Result<Vec<OwnedObjectPath>> {
        let svc = self.proxy(SERVICE_PATH, SERVICE)?;
        let (mut unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) =
            svc.call("SearchItems", &(attrs))?;
        if !locked.is_empty() {
            let (now, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) =
                svc.call("Unlock", &(&locked))?;
            unlocked.extend(now);
            if !is_none_path(&prompt) {
                self.prompt(&prompt)?;
                let (again, _): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) =
                    svc.call("SearchItems", &(attrs))?;
                unlocked = again;
            }
        }
        Ok(unlocked)
    }

    /// All logins saved for `origin`, sorted by user name.
    pub fn logins(&self, origin: &str) -> Result<Vec<Login>> {
        self.find(&query(origin, None))
    }

    /// Every Helix login in the keyring.
    pub fn all_logins(&self) -> Result<Vec<Login>> {
        self.find(&HashMap::from([(ATTR_SCHEMA.to_string(), SCHEMA.to_string())]))
    }

    fn find(&self, attrs: &HashMap<String, String>) -> Result<Vec<Login>> {
        let paths = self.search_paths(attrs)?;
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let svc = self.proxy(SERVICE_PATH, SERVICE)?;
        let secrets: HashMap<OwnedObjectPath, Secret> =
            svc.call("GetSecrets", &(&paths, &self.session))?;
        let mut out = Vec::new();
        for path in &paths {
            let Some((_, _, value, _)) = secrets.get(path) else {
                continue;
            };
            let item = self.proxy(path.as_str(), ITEM)?;
            let a: HashMap<String, String> = item.get_property("Attributes")?;
            if a.get(ATTR_SCHEMA).map(String::as_str) != Some(SCHEMA) {
                continue;
            }
            out.push(Login {
                origin: a.get(ATTR_ORIGIN).cloned().unwrap_or_default(),
                target_origin: a.get(ATTR_TARGET).cloned().unwrap_or_default(),
                username: a.get(ATTR_USERNAME).cloned().unwrap_or_default(),
                password: String::from_utf8_lossy(value).into_owned(),
            });
        }
        out.sort_by(|a, b| a.origin.cmp(&b.origin).then(a.username.cmp(&b.username)));
        Ok(out)
    }

    fn default_collection(&self) -> Result<OwnedObjectPath> {
        let svc = self.proxy(SERVICE_PATH, SERVICE)?;
        let path: OwnedObjectPath = svc.call("ReadAlias", &("default"))?;
        if !is_none_path(&path) {
            return Ok(path);
        }
        // No default collection yet: ask the service to create one.
        let label = HashMap::from([(
            "org.freedesktop.Secret.Collection.Label",
            Value::from("Login"),
        )]);
        let (created, prompt): (OwnedObjectPath, OwnedObjectPath) =
            svc.call("CreateCollection", &(label, "default"))?;
        if !is_none_path(&created) {
            return Ok(created);
        }
        self.prompt(&prompt)?;
        let path: OwnedObjectPath = svc.call("ReadAlias", &("default"))?;
        if is_none_path(&path) {
            return Err(Error::Unavailable);
        }
        Ok(path)
    }

    /// Saves a login, replacing an existing one for the same origin and
    /// user name.
    pub fn store(&self, login: &Login) -> Result<()> {
        let collection = self.default_collection()?;
        let col = self.proxy(collection.as_str(), COLLECTION)?;
        if col.get_property::<bool>("Locked").unwrap_or(false) {
            let svc = self.proxy(SERVICE_PATH, SERVICE)?;
            let (_, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) =
                svc.call("Unlock", &(vec![&collection]))?;
            self.prompt(&prompt)?;
        }
        let props: HashMap<&str, Value> = HashMap::from([
            ("org.freedesktop.Secret.Item.Label", Value::from(login.label())),
            (
                "org.freedesktop.Secret.Item.Attributes",
                Value::from(login.attributes()),
            ),
        ]);
        let secret = (
            &self.session,
            Vec::<u8>::new(),
            login.password.as_bytes().to_vec(),
            CONTENT_TYPE,
        );
        let (_item, prompt): (OwnedObjectPath, OwnedObjectPath) =
            col.call("CreateItem", &(props, secret, true))?;
        self.prompt(&prompt)
    }

    /// Deletes the login for `origin` / `username`.
    pub fn delete(&self, origin: &str, username: &str) -> Result<()> {
        for path in self.search_paths(&query(origin, Some(username)))? {
            let item = self.proxy(path.as_str(), ITEM)?;
            let prompt: OwnedObjectPath = item.call("Delete", &())?;
            self.prompt(&prompt)?;
        }
        Ok(())
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if let Ok(p) = Proxy::new(&self.conn, BUS, self.session.as_str(), "org.freedesktop.Secret.Session") {
            let _: zbus::Result<()> = p.call("Close", &());
        }
    }
}

#[cfg(test)]
pub mod mock {
    //! An in-process mock Secret Service served over a private
    //! peer-to-peer connection: tests never touch a real keyring or bus.

    use super::*;
    use std::sync::{Arc, Mutex};
    use zbus::object_server::SignalEmitter;
    use zbus::{interface, ObjectServer};

    const DEFAULT_ALIAS: &str = "/org/freedesktop/secrets/aliases/default";

    #[derive(Default)]
    pub struct Store {
        pub items: Vec<MockItem>,
        pub next: u32,
        pub locked: bool,
        pub prompts_shown: u32,
        pub dismiss_prompts: bool,
    }

    pub type Shared = Arc<Mutex<Store>>;
    /// id, attributes, secret, content type.
    pub type MockItem = (u32, HashMap<String, String>, Vec<u8>, String);

    fn item_path(id: u32) -> String {
        format!("/org/freedesktop/secrets/collection/login/{id}")
    }

    fn matches(attrs: &HashMap<String, String>, q: &HashMap<String, String>) -> bool {
        q.iter().all(|(k, v)| attrs.get(k) == Some(v))
    }

    struct Service(Shared);
    struct Collection(Shared);
    struct Item(Shared, u32);
    struct Prompt(Shared, Option<u32>);

    #[interface(name = "org.freedesktop.Secret.Service")]
    impl Service {
        fn open_session(&self, algorithm: &str, _input: Value<'_>) -> zbus::fdo::Result<(OwnedValue, OwnedObjectPath)> {
            if algorithm != "plain" {
                return Err(zbus::fdo::Error::NotSupported("algorithm".into()));
            }
            Ok((
                OwnedValue::try_from(Value::from("")).unwrap(),
                OwnedObjectPath::try_from("/org/freedesktop/secrets/session/1").unwrap(),
            ))
        }

        fn search_items(&self, attributes: HashMap<String, String>) -> (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) {
            let s = self.0.lock().unwrap();
            let found: Vec<OwnedObjectPath> = s
                .items
                .iter()
                .filter(|(_, a, _, _)| matches(a, &attributes))
                .map(|(id, ..)| OwnedObjectPath::try_from(item_path(*id)).unwrap())
                .collect();
            if s.locked {
                (Vec::new(), found)
            } else {
                (found, Vec::new())
            }
        }

        async fn unlock(
            &self,
            _objects: Vec<OwnedObjectPath>,
            #[zbus(object_server)] server: &ObjectServer,
        ) -> (Vec<OwnedObjectPath>, OwnedObjectPath) {
            let path = "/org/freedesktop/secrets/prompt/unlock";
            server.at(path, Prompt(self.0.clone(), None)).await.unwrap();
            (Vec::new(), OwnedObjectPath::try_from(path).unwrap())
        }

        fn get_secrets(
            &self,
            items: Vec<OwnedObjectPath>,
            session: OwnedObjectPath,
        ) -> HashMap<OwnedObjectPath, (OwnedObjectPath, Vec<u8>, Vec<u8>, String)> {
            let s = self.0.lock().unwrap();
            s.items
                .iter()
                .filter(|(id, ..)| items.iter().any(|p| p.as_str() == item_path(*id)))
                .map(|(id, _, v, ct)| {
                    (
                        OwnedObjectPath::try_from(item_path(*id)).unwrap(),
                        (session.clone(), Vec::new(), v.clone(), ct.clone()),
                    )
                })
                .collect()
        }

        fn read_alias(&self, name: &str) -> OwnedObjectPath {
            let p = if name == "default" { DEFAULT_ALIAS } else { "/" };
            OwnedObjectPath::try_from(p).unwrap()
        }
    }

    #[interface(name = "org.freedesktop.Secret.Collection")]
    impl Collection {
        #[zbus(property)]
        fn locked(&self) -> bool {
            self.0.lock().unwrap().locked
        }

        async fn create_item(
            &self,
            properties: HashMap<String, OwnedValue>,
            secret: (OwnedObjectPath, Vec<u8>, Vec<u8>, String),
            replace: bool,
            #[zbus(object_server)] server: &ObjectServer,
        ) -> zbus::fdo::Result<(OwnedObjectPath, OwnedObjectPath)> {
            let attrs: HashMap<String, String> = properties
                .get("org.freedesktop.Secret.Item.Attributes")
                .and_then(|v| HashMap::<String, String>::try_from(v.try_clone().ok()?).ok())
                .ok_or_else(|| zbus::fdo::Error::InvalidArgs("attributes".into()))?;
            let id = {
                let mut s = self.0.lock().unwrap();
                if s.locked {
                    return Err(zbus::fdo::Error::AccessDenied("locked".into()));
                }
                if replace {
                    if let Some(item) = s.items.iter_mut().find(|(_, a, _, _)| *a == attrs) {
                        item.2 = secret.2.clone();
                        return Ok((
                            OwnedObjectPath::try_from(item_path(item.0)).unwrap(),
                            OwnedObjectPath::try_from("/").unwrap(),
                        ));
                    }
                }
                s.next += 1;
                let id = s.next;
                s.items.push((id, attrs, secret.2, secret.3));
                id
            };
            server.at(item_path(id), Item(self.0.clone(), id)).await.map_err(zbus::fdo::Error::from)?;
            Ok((
                OwnedObjectPath::try_from(item_path(id)).unwrap(),
                OwnedObjectPath::try_from("/").unwrap(),
            ))
        }
    }

    #[interface(name = "org.freedesktop.Secret.Item")]
    impl Item {
        #[zbus(property)]
        fn attributes(&self) -> HashMap<String, String> {
            let s = self.0.lock().unwrap();
            s.items
                .iter()
                .find(|(id, ..)| *id == self.1)
                .map(|(_, a, _, _)| a.clone())
                .unwrap_or_default()
        }

        /// Deleting goes through a confirmation prompt, to exercise
        /// prompt handling.
        async fn delete(&self, #[zbus(object_server)] server: &ObjectServer) -> OwnedObjectPath {
            let path = format!("/org/freedesktop/secrets/prompt/delete{}", self.1);
            server.at(path.as_str(), Prompt(self.0.clone(), Some(self.1))).await.unwrap();
            OwnedObjectPath::try_from(path).unwrap()
        }
    }

    #[interface(name = "org.freedesktop.Secret.Prompt")]
    impl Prompt {
        async fn prompt(&self, _window_id: &str, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) {
            let dismissed = {
                let mut s = self.0.lock().unwrap();
                s.prompts_shown += 1;
                if !s.dismiss_prompts {
                    match self.1 {
                        Some(id) => s.items.retain(|(i, ..)| *i != id),
                        None => s.locked = false,
                    }
                }
                s.dismiss_prompts
            };
            let _ = Self::completed(&emitter, dismissed, Value::from("")).await;
        }

        #[zbus(signal)]
        async fn completed(emitter: &SignalEmitter<'_>, dismissed: bool, result: Value<'_>) -> zbus::Result<()>;
    }

    /// Starts the mock; returns a client connection and the shared store.
    pub fn start() -> (Connection, Shared, Connection) {
        let shared: Shared = Arc::default();
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        let guid = zbus::Guid::generate();
        let s2 = shared.clone();
        let server = std::thread::spawn(move || {
            zbus::blocking::connection::Builder::async_io_unix_stream(a)
                .server(guid)
                .unwrap()
                .p2p()
                .serve_at(SERVICE_PATH, Service(s2.clone()))
                .unwrap()
                .serve_at(DEFAULT_ALIAS, Collection(s2))
                .unwrap()
                .build()
                .unwrap()
        });
        let client = zbus::blocking::connection::Builder::async_io_unix_stream(b)
            .p2p()
            .build()
            .unwrap();
        let server = server.join().unwrap();
        (client, shared, server)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login(origin: &str, user: &str, pw: &str) -> Login {
        Login {
            origin: origin.into(),
            target_origin: origin.into(),
            username: user.into(),
            password: pw.into(),
        }
    }

    #[test]
    fn attributes_and_labels() {
        let l = login("https://example.com", "ada", "pw");
        let a = l.attributes();
        assert_eq!(a[ATTR_SCHEMA], "org.helix.Web.Password");
        assert_eq!(a[ATTR_ORIGIN], "https://example.com");
        assert_eq!(a[ATTR_USERNAME], "ada");
        assert_eq!(l.label(), "Password for ada on example.com");
        assert_eq!(login("https://e:8443", "", "x").label(), "Password for e:8443");
        let q = query("https://example.com", None);
        assert_eq!(q.len(), 2);
        assert_eq!(query("o", Some("u"))[ATTR_USERNAME], "u");
    }

    #[test]
    fn store_search_replace_delete_against_mock() {
        let (conn, store, _server) = mock::start();
        let c = Client::new(conn).unwrap();
        assert!(c.logins("https://example.com").unwrap().is_empty());

        c.store(&login("https://example.com", "ada", "first")).unwrap();
        c.store(&login("https://example.com", "bob", "pw-b")).unwrap();
        c.store(&login("https://other.org", "ada", "pw-o")).unwrap();
        // Same origin and user: replaced, not duplicated.
        c.store(&login("https://example.com", "ada", "second")).unwrap();
        assert_eq!(store.lock().unwrap().items.len(), 3);

        let got = c.logins("https://example.com").unwrap();
        assert_eq!(
            got,
            vec![
                login("https://example.com", "ada", "second"),
                login("https://example.com", "bob", "pw-b"),
            ]
        );
        assert_eq!(c.all_logins().unwrap().len(), 3);

        // Delete runs a prompt and waits for Completed.
        c.delete("https://example.com", "bob").unwrap();
        assert_eq!(store.lock().unwrap().prompts_shown, 1);
        assert_eq!(c.logins("https://example.com").unwrap().len(), 1);

        store.lock().unwrap().dismiss_prompts = true;
        assert!(matches!(c.delete("https://example.com", "ada"), Err(Error::Dismissed)));
        assert_eq!(c.logins("https://example.com").unwrap().len(), 1);
    }

    #[test]
    fn locked_items_are_unlocked_through_a_prompt() {
        let (conn, store, _server) = mock::start();
        let c = Client::new(conn).unwrap();
        c.store(&login("https://example.com", "ada", "pw")).unwrap();
        store.lock().unwrap().locked = true;
        let got = c.logins("https://example.com").unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].password, "pw");
        assert!(!store.lock().unwrap().locked);
    }
}
