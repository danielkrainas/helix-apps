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
//!
//! Secrets travel encrypted (`dh-ietf1024-sha256-aes128-cbc-pkcs7`, see
//! `sscrypto`) unless the service rejects that algorithm, in which case the
//! client falls back to `plain`.

use crate::sscrypto;
use std::collections::HashMap;
use zeroize::{Zeroize, Zeroizing};
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
    /// The service sent a secret that does not decrypt, or a bad session
    /// key.
    Crypto,
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
            Error::Crypto => write!(f, "could not set up or use the encrypted Secret Service session"),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

pub struct Client {
    conn: Connection,
    session: OwnedObjectPath,
    /// AES key of an encrypted session; `None` for `plain`.
    key: Option<sscrypto::Key>,
}

/// Whether an `OpenSession` failure means "algorithm not supported" (as
/// opposed to the service or bus being unavailable).
fn is_rejection(e: &zbus::Error) -> bool {
    matches!(e, zbus::Error::MethodError(..) | zbus::Error::FDO(_))
}

fn is_none_path(p: &ObjectPath<'_>) -> bool {
    p.as_str() == "/"
}

impl Client {
    /// Connects to the Secret Service on the user's session bus.
    pub fn session_bus() -> Result<Client> {
        Client::new(Connection::session()?)
    }

    /// Opens a transfer session on `conn` (the session bus, or a private
    /// connection in tests): encrypted, or `plain` if the service rejects
    /// the encrypted algorithm.
    pub fn new(conn: Connection) -> Result<Client> {
        let svc = Proxy::new(&conn, BUS, SERVICE_PATH, SERVICE)?;
        let pair = sscrypto::KeyPair::generate();
        let reply: zbus::Result<(OwnedValue, OwnedObjectPath)> = svc.call(
            "OpenSession",
            &(sscrypto::ALGORITHM, Value::from(pair.public().to_vec())),
        );
        match reply {
            Ok((output, session)) => {
                let key = Vec::<u8>::try_from(output)
                    .ok()
                    .and_then(|peer| pair.derive(&peer));
                let client = Client { conn, session, key };
                if client.key.is_none() {
                    // Dropping closes the half-open session.
                    return Err(Error::Crypto);
                }
                Ok(client)
            }
            Err(e) if is_rejection(&e) => {
                eprintln!(
                    "helix-web: Secret Service rejected {} ({e}); using plain transfer",
                    sscrypto::ALGORITHM
                );
                let (_out, session): (OwnedValue, OwnedObjectPath) =
                    svc.call("OpenSession", &("plain", Value::from("")))?;
                Ok(Client { conn, session, key: None })
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Whether secrets travel encrypted in this session.
    #[cfg(test)]
    pub fn encrypted(&self) -> bool {
        self.key.is_some()
    }

    /// (parameters, value) for a secret sent to the service.
    fn encode(&self, plain: &[u8]) -> (Vec<u8>, Zeroizing<Vec<u8>>) {
        match &self.key {
            Some(key) => {
                let (iv, ct) = sscrypto::encrypt(key, plain);
                (iv, Zeroizing::new(ct))
            }
            None => (Vec::new(), Zeroizing::new(plain.to_vec())),
        }
    }

    /// The plaintext of a secret received from the service.
    fn decode(&self, params: &[u8], value: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        match &self.key {
            Some(key) => sscrypto::decrypt(key, params, value).ok_or(Error::Crypto),
            None => Ok(Zeroizing::new(value.to_vec())),
        }
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
        let mut secrets: HashMap<OwnedObjectPath, Secret> =
            svc.call("GetSecrets", &(&paths, &self.session))?;
        let result = self.collect(&paths, &secrets);
        // In a plain session the values are the passwords themselves.
        for (_, _, value, _) in secrets.values_mut() {
            value.zeroize();
        }
        let mut out = result?;
        out.sort_by(|a, b| a.origin.cmp(&b.origin).then(a.username.cmp(&b.username)));
        Ok(out)
    }

    fn collect(
        &self,
        paths: &[OwnedObjectPath],
        secrets: &HashMap<OwnedObjectPath, Secret>,
    ) -> Result<Vec<Login>> {
        let mut out = Vec::new();
        for path in paths {
            let Some((_, params, value, _)) = secrets.get(path) else {
                continue;
            };
            let item = self.proxy(path.as_str(), ITEM)?;
            let a: HashMap<String, String> = item.get_property("Attributes")?;
            if a.get(ATTR_SCHEMA).map(String::as_str) != Some(SCHEMA) {
                continue;
            }
            let plain = self.decode(params, value)?;
            out.push(Login {
                origin: a.get(ATTR_ORIGIN).cloned().unwrap_or_default(),
                target_origin: a.get(ATTR_TARGET).cloned().unwrap_or_default(),
                username: a.get(ATTR_USERNAME).cloned().unwrap_or_default(),
                password: String::from_utf8_lossy(&plain).into_owned(),
            });
        }
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
        let (params, value) = self.encode(login.password.as_bytes());
        let secret = (&self.session, params, &value[..], CONTENT_TYPE);
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
        /// Answer `OpenSession` for the encrypted algorithm with
        /// `NotSupported`, like a service that only speaks `plain`.
        pub reject_dh: bool,
        /// Algorithm of the most recently opened session.
        pub algorithm: String,
        /// Session key of the encrypted session (the mock has one session
        /// at a time).
        pub key: Option<[u8; 16]>,
        /// Every secret value as it crossed the connection, both ways.
        pub wire: Vec<Vec<u8>>,
    }

    impl Store {
        fn send(&mut self, plain: &[u8]) -> (Vec<u8>, Vec<u8>) {
            let (iv, value) = match self.key {
                Some(k) => sscrypto::encrypt(&Zeroizing::new(k), plain),
                None => (Vec::new(), plain.to_vec()),
            };
            self.wire.push(value.clone());
            (iv, value)
        }

        fn receive(&mut self, params: &[u8], value: &[u8]) -> Option<Vec<u8>> {
            self.wire.push(value.to_vec());
            match self.key {
                Some(k) => sscrypto::decrypt(&Zeroizing::new(k), params, value).map(|p| p.to_vec()),
                None => Some(value.to_vec()),
            }
        }
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
        fn open_session(&self, algorithm: &str, input: Value<'_>) -> zbus::fdo::Result<(OwnedValue, OwnedObjectPath)> {
            let mut s = self.0.lock().unwrap();
            let output = match algorithm {
                "plain" => {
                    s.key = None;
                    Value::from("")
                }
                sscrypto::ALGORITHM if !s.reject_dh => {
                    let peer = Vec::<u8>::try_from(input)
                        .map_err(|_| zbus::fdo::Error::InvalidArgs("input".into()))?;
                    let pair = sscrypto::KeyPair::generate();
                    let key = pair
                        .derive(&peer)
                        .ok_or_else(|| zbus::fdo::Error::InvalidArgs("public key".into()))?;
                    s.key = Some(*key);
                    Value::from(pair.public().to_vec())
                }
                _ => return Err(zbus::fdo::Error::NotSupported("algorithm".into())),
            };
            s.algorithm = algorithm.to_string();
            Ok((
                OwnedValue::try_from(output).unwrap(),
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
            let mut s = self.0.lock().unwrap();
            let wanted: Vec<(u32, Vec<u8>, String)> = s
                .items
                .iter()
                .filter(|(id, ..)| items.iter().any(|p| p.as_str() == item_path(*id)))
                .map(|(id, _, v, ct)| (*id, v.clone(), ct.clone()))
                .collect();
            wanted
                .into_iter()
                .map(|(id, v, ct)| {
                    let (params, value) = s.send(&v);
                    (
                        OwnedObjectPath::try_from(item_path(id)).unwrap(),
                        (session.clone(), params, value, ct),
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
                let plain = s
                    .receive(&secret.1, &secret.2)
                    .ok_or_else(|| zbus::fdo::Error::InvalidArgs("secret".into()))?;
                if replace {
                    if let Some(item) = s.items.iter_mut().find(|(_, a, _, _)| *a == attrs) {
                        item.2 = plain;
                        return Ok((
                            OwnedObjectPath::try_from(item_path(item.0)).unwrap(),
                            OwnedObjectPath::try_from("/").unwrap(),
                        ));
                    }
                }
                s.next += 1;
                let id = s.next;
                s.items.push((id, attrs, plain, secret.3));
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
    fn secrets_travel_encrypted_by_default() {
        let (conn, store, _server) = mock::start();
        let c = Client::new(conn).unwrap();
        assert!(c.encrypted());
        assert_eq!(store.lock().unwrap().algorithm, sscrypto::ALGORITHM);

        c.store(&login("https://example.com", "ada", "hunter2")).unwrap();
        // The mock decrypted it with the negotiated key...
        assert_eq!(store.lock().unwrap().items[0].2, b"hunter2");
        let got = c.logins("https://example.com").unwrap();
        assert_eq!(got[0].password, "hunter2");
        // ...and nothing crossed the connection in the clear.
        let wire = store.lock().unwrap().wire.clone();
        assert_eq!(wire.len(), 2);
        for value in &wire {
            assert_eq!(value.len(), 16);
            assert!(!value.windows(7).any(|w| w == b"hunter2"));
        }
        // Fresh IVs: the same password encrypts differently each time.
        assert_ne!(wire[0], wire[1]);
    }

    #[test]
    fn a_corrupted_secret_is_an_error() {
        let (conn, store, _server) = mock::start();
        let c = Client::new(conn).unwrap();
        c.store(&login("https://example.com", "ada", "pw")).unwrap();
        // The service now answers in the clear (no IV): never accepted as
        // a password in an encrypted session.
        store.lock().unwrap().key = None;
        assert!(matches!(c.logins("https://example.com"), Err(Error::Crypto)));
    }

    #[test]
    fn falls_back_to_plain_when_the_algorithm_is_rejected() {
        let (conn, store, _server) = mock::start();
        store.lock().unwrap().reject_dh = true;
        let c = Client::new(conn).unwrap();
        assert!(!c.encrypted());
        assert_eq!(store.lock().unwrap().algorithm, "plain");
        c.store(&login("https://example.com", "ada", "pw")).unwrap();
        assert_eq!(c.logins("https://example.com").unwrap()[0].password, "pw");
        assert_eq!(store.lock().unwrap().wire[0], b"pw");
    }

    /// Interoperability check against a real Secret Service. Only runs
    /// when `HELIX_WEB_SECRET_INTEROP=1`, inside a private
    /// `dbus-run-session` with a throwaway keyring daemon (never the
    /// user's own keyring).
    #[test]
    #[ignore]
    fn interop_with_a_private_secret_service() {
        if std::env::var("HELIX_WEB_SECRET_INTEROP").as_deref() != Ok("1") {
            return;
        }
        let c = Client::session_bus().unwrap();
        assert!(c.encrypted());
        let l = login("https://interop.example", "ada", "pässwörd-0123456789abcdef");
        c.store(&l).unwrap();
        assert_eq!(c.logins("https://interop.example").unwrap(), vec![l]);
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
