//! Helix shell extensions through `org.helix.Extensions1` (helix-desktop-spec
//! `contracts/extensions/README.md`, "Session bus"): what the Extensions
//! panel shows and changes. Blocking zbus; call only from worker threads.

use std::collections::HashMap;

use zbus::blocking::Connection;
use zbus::zvariant::{OwnedValue, Value};

pub const BUS: &str = "org.gnome.Shell";
pub const PATH: &str = "/org/helix/Extensions1";
pub const IFACE: &str = "org.helix.Extensions1";

/// What an extension's permission lets it do (as the shell words it).
pub fn permission_text(name: &str) -> &str {
    const TEXTS: [(&str, &str); 14] = [
        ("notifications", "show notifications"),
        ("inhibit-idle", "keep the screen on"),
        ("launch-apps", "start apps"),
        ("popover", "show its own panels"),
        ("network", "use the network"),
        ("files:home", "read and change all your files"),
        ("files:documents", "read and change your Documents"),
        ("files:downloads", "read and change your Downloads"),
        ("files:music", "read and change your Music"),
        ("files:pictures", "read and change your Pictures"),
        ("files:videos", "read and change your Videos"),
        ("files:desktop", "read and change your Desktop"),
        ("session-bus", "talk to your other programs"),
        ("system-bus", "talk to system services"),
    ];
    TEXTS.iter().find(|(n, _)| *n == name).map_or(name, |(_, t)| t)
}

/// One extension, as `ListExtensions` describes it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ext {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    /// `system`, `user` or `development`.
    pub kind: String,
    pub enabled: bool,
    pub approved: bool,
    pub state: String,
    pub error: String,
    pub permissions: Vec<String>,
    /// Command id, title, accelerator.
    pub commands: Vec<(String, String, String)>,
    pub sandboxed: bool,
}

impl Ext {
    pub fn development(&self) -> bool {
        self.kind == "development"
    }

    /// The switch: on when enabled and approved.
    pub fn on(&self) -> bool {
        self.enabled && self.approved
    }

    /// The row's subtitle: version and state.
    pub fn subtitle(&self) -> String {
        let state = state_text(self);
        let mut parts: Vec<String> = Vec::new();
        if !self.version.is_empty() {
            parts.push(self.version.clone());
        }
        if self.development() {
            parts.push("Development".into());
        }
        parts.push(state);
        parts.join(" — ")
    }

    /// The *Access* row: title and the lines below it.
    pub fn access(&self) -> (String, String) {
        let can: Vec<&str> = self.permissions.iter().map(|p| permission_text(p)).collect();
        let below = if can.is_empty() { String::new() } else { format!("Can: {}", can.join(", ")) };
        if self.sandboxed {
            ("Sandboxed".into(), below)
        } else {
            ("Full access: it can do anything you can".into(), below)
        }
    }

    /// It has a problem worth a *Problem* row.
    pub fn troubled(&self) -> bool {
        matches!(self.state.as_str(), "invalid" | "crashed")
    }
}

pub fn state_text(e: &Ext) -> String {
    match e.state.as_str() {
        "running" => "Running".into(),
        "starting" => "Starting".into(),
        "stopped" => "Not running".into(),
        "disabled" if e.error == "Off for this session" => "Off for this session".into(),
        "disabled" => "Off".into(),
        "needs-approval" => "Needs approval".into(),
        "invalid" => format!("Cannot run: {}", e.error),
        "crashed" if e.error.is_empty() => "Stopped working".into(),
        "crashed" => format!("Stopped working: {}", e.error),
        other => other.into(),
    }
}

fn get<T: TryFrom<OwnedValue>>(entry: &HashMap<String, OwnedValue>, key: &str) -> Option<T> {
    entry.get(key).and_then(|v| T::try_from(v.clone()).ok())
}

/// The entries of `ListExtensions`, user and system ones by name, then
/// development ones.
pub fn parse_list(list: HashMap<String, HashMap<String, OwnedValue>>) -> Vec<Ext> {
    let mut out: Vec<Ext> = list
        .into_iter()
        .map(|(id, e)| Ext {
            name: get::<String>(&e, "name").filter(|n| !n.is_empty()).unwrap_or_else(|| id.clone()),
            id,
            description: get(&e, "description").unwrap_or_default(),
            version: get(&e, "version").unwrap_or_default(),
            kind: get(&e, "type").unwrap_or_default(),
            enabled: get(&e, "enabled").unwrap_or(false),
            approved: get(&e, "approved").unwrap_or(false),
            state: get(&e, "state").unwrap_or_default(),
            error: get(&e, "error").unwrap_or_default(),
            permissions: get(&e, "permissions").unwrap_or_default(),
            commands: get(&e, "commands").unwrap_or_default(),
            sandboxed: get(&e, "sandboxed").unwrap_or(false),
        })
        .collect();
    out.sort_by_key(|e| (e.development(), e.name.to_lowercase(), e.id.clone()));
    out
}

/// A setting's value.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Bool(bool),
    Int(i64),
    Double(f64),
    Text(String),
    List(Vec<String>),
}

impl Val {
    pub fn to_value(&self) -> Value<'static> {
        match self {
            Val::Bool(b) => Value::from(*b),
            Val::Int(i) => Value::from(*i),
            Val::Double(d) => Value::from(*d),
            Val::Text(s) => Value::from(s.clone()),
            Val::List(l) => Value::from(l.clone()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bool,
    Int,
    Double,
    Text,
    Enum,
    List,
}

/// One declared setting (`GetSettings`).
#[derive(Debug, Clone, PartialEq)]
pub struct Setting {
    pub key: String,
    pub kind: Kind,
    pub value: Val,
    pub label: String,
    pub help: String,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub choices: Vec<String>,
}

impl Setting {
    /// Spin-row range.
    pub fn range(&self) -> (f64, f64) {
        match self.kind {
            Kind::Int => (self.min.unwrap_or(i32::MIN as f64), self.max.unwrap_or(i32::MAX as f64)),
            _ => (self.min.unwrap_or(-1e9), self.max.unwrap_or(1e9)),
        }
    }

    /// Spin-row step.
    pub fn step(&self) -> f64 {
        match (self.kind, self.min, self.max) {
            (Kind::Int, _, _) => 1.0,
            (_, Some(lo), Some(hi)) if hi > lo => (hi - lo) / 20.0,
            _ => 0.1,
        }
    }
}

pub type SettingEntry = (String, String, OwnedValue, HashMap<String, OwnedValue>);

pub fn parse_settings(entries: Vec<SettingEntry>) -> Vec<Setting> {
    entries
        .into_iter()
        .filter_map(|(key, ty, value, details)| {
            let (kind, value) = match ty.as_str() {
                "bool" => (Kind::Bool, Val::Bool(bool::try_from(value).ok()?)),
                "int" => (Kind::Int, Val::Int(i64::try_from(value).ok()?)),
                "double" => (Kind::Double, Val::Double(f64::try_from(value).ok()?)),
                "string" => (Kind::Text, Val::Text(String::try_from(value).ok()?)),
                "enum" => (Kind::Enum, Val::Text(String::try_from(value).ok()?)),
                "string-list" => (Kind::List, Val::List(Vec::<String>::try_from(value).ok()?)),
                _ => return None,
            };
            Some(Setting {
                label: get::<String>(&details, "label").filter(|l| !l.is_empty()).unwrap_or_else(|| key.clone()),
                help: get(&details, "help").unwrap_or_default(),
                min: get(&details, "min"),
                max: get(&details, "max"),
                choices: get(&details, "choices").unwrap_or_default(),
                key,
                kind,
                value,
            })
        })
        .collect()
}

/// A string-list typed as comma-separated text.
pub fn split_list(text: &str) -> Vec<String> {
    text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect()
}

/// Whether a key combination may become a command's key: with Ctrl, Alt or
/// Super, or a function key alone (`key` is the key's name, as GDK names it).
pub fn shortcut_allowed(key: &str, ctrl_alt_super: bool) -> bool {
    const MODIFIERS: [&str; 9] = ["Shift", "Control", "Alt", "Super", "Meta", "Hyper", "ISO_Level3", "Caps_Lock", "Num_Lock"];
    if key.is_empty() || MODIFIERS.iter().any(|m| key.starts_with(m)) {
        return false;
    }
    let function = key.len() > 1 && key.starts_with('F') && key[1..].parse::<u32>().is_ok();
    ctrl_alt_super || function
}

/// How a call to the shell failed.
#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    /// The user (or the shell) said no.
    Denied,
    /// Anything else, with its message.
    Other(String),
}

fn failure(e: zbus::Error) -> Failure {
    match e {
        zbus::Error::MethodError(name, _, _) if name.as_str() == "org.freedesktop.DBus.Error.AccessDenied" => Failure::Denied,
        zbus::Error::MethodError(_, Some(msg), _) => Failure::Other(msg),
        other => Failure::Other(other.to_string()),
    }
}

fn session() -> Result<Connection, Failure> {
    Connection::session().map_err(failure)
}

fn call<B, R>(method: &str, body: &B) -> Result<R, Failure>
where
    B: serde::ser::Serialize + zbus::zvariant::DynamicType,
    R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
{
    let conn = session()?;
    let reply = conn.call_method(Some(BUS), PATH, Some(IFACE), method, body).map_err(failure)?;
    reply.body().deserialize().map_err(|e| Failure::Other(e.to_string()))
}

/// The extensions; `None` when the shell does not serve them.
pub fn list() -> Option<Vec<Ext>> {
    call::<_, HashMap<String, HashMap<String, OwnedValue>>>("ListExtensions", &()).ok().map(parse_list)
}

pub fn settings(id: &str) -> Vec<Setting> {
    call::<_, Vec<SettingEntry>>("GetSettings", &(id,)).map(parse_settings).unwrap_or_default()
}

/// Turn it on, approving what it asks for: the shell may ask the user
/// first, so this can take long.
pub fn enable(id: &str) -> Result<(), Failure> {
    call::<_, ()>("Enable", &(id, true))
}

pub fn disable(id: &str) -> Result<(), Failure> {
    call::<_, ()>("Disable", &(id,))
}

pub fn restart(id: &str) -> Result<(), Failure> {
    call::<_, ()>("Restart", &(id,))
}

pub fn log(id: &str) -> Vec<String> {
    call::<_, Vec<String>>("GetLog", &(id,)).unwrap_or_default()
}

pub fn set_setting(id: &str, key: &str, value: &Val) -> Result<(), Failure> {
    call::<_, ()>("SetSetting", &(id, key, value.to_value()))
}

/// Set a command's key (`""`: none).
pub fn bind(command: &str, accelerator: &str) -> Result<(), Failure> {
    call::<_, ()>("BindCommand", &(command, accelerator))
}

/// Calls `changed` (from this thread) whenever the extensions change or the
/// shell comes or goes; returns when the session bus goes away.
pub fn watch(mut changed: impl FnMut() -> bool) {
    let Ok(conn) = Connection::session() else { return };
    let rules = [
        zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).interface(IFACE).and_then(|b| b.member("Changed")).map(|b| b.build()),
        zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("org.freedesktop.DBus")
            .and_then(|b| b.member("NameOwnerChanged"))
            .and_then(|b| b.arg(0, BUS))
            .map(|b| b.build()),
    ];
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(&conn) else { return };
    for rule in rules.into_iter().flatten() {
        let _ = dbus.add_match_rule(rule);
    }
    for message in zbus::blocking::MessageIterator::from(&conn) {
        let Ok(message) = message else { continue };
        let header = message.header();
        let interesting = header.message_type() == zbus::message::Type::Signal
            && matches!(header.member().map(|m| m.as_str()), Some("Changed" | "NameOwnerChanged"));
        if interesting && !changed() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v<'a>(x: impl Into<Value<'a>>) -> OwnedValue {
        OwnedValue::try_from(x.into()).unwrap()
    }

    #[test]
    fn list_order_and_texts() {
        let entry = |name: &str, kind: &str, state: &str, error: &str| {
            HashMap::from([
                ("name".to_owned(), v(name)),
                ("type".to_owned(), v(kind)),
                ("state".to_owned(), v(state)),
                ("error".to_owned(), v(error)),
                ("version".to_owned(), v("1.0")),
                ("enabled".to_owned(), v(true)),
                ("approved".to_owned(), v(true)),
                ("permissions".to_owned(), v(vec!["inhibit-idle".to_owned(), "odd".to_owned()])),
            ])
        };
        let list = parse_list(HashMap::from([
            ("org.x.Dev".to_owned(), entry("Alpha", "development", "running", "")),
            ("org.x.Zed".to_owned(), entry("zed", "user", "crashed", "boom")),
            ("org.x.Cup".to_owned(), entry("Caffeine", "system", "disabled", "Off for this session")),
        ]));
        let names: Vec<&str> = list.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Caffeine", "zed", "Alpha"], "development ones last");
        assert_eq!(list[0].subtitle(), "1.0 — Off for this session");
        assert_eq!(list[1].subtitle(), "1.0 — Stopped working: boom");
        assert!(list[1].troubled());
        assert_eq!(list[2].subtitle(), "1.0 — Development — Running");
        assert_eq!(list[0].access(), ("Full access: it can do anything you can".to_owned(), "Can: keep the screen on, odd".to_owned()));
        let sandboxed = Ext { sandboxed: true, ..Default::default() };
        assert_eq!(sandboxed.access(), ("Sandboxed".to_owned(), String::new()));
        assert_eq!(state_text(&Ext { state: "needs-approval".into(), ..Default::default() }), "Needs approval");
    }

    #[test]
    fn settings_parsed() {
        let details = |pairs: Vec<(&str, OwnedValue)>| pairs.into_iter().map(|(k, x)| (k.to_owned(), x)).collect::<HashMap<_, _>>();
        let s = parse_settings(vec![
            ("mode".into(), "enum".into(), v("slow"), details(vec![("label", v("Mode")), ("choices", v(vec!["fast".to_owned(), "slow".to_owned()]))])),
            ("step".into(), "int".into(), v(2i64), details(vec![("label", v("Step")), ("min", v(1.0)), ("max", v(9.0))])),
            ("level".into(), "double".into(), v(0.5), details(vec![("min", v(0.0)), ("max", v(2.0))])),
            ("names".into(), "string-list".into(), v(vec!["a".to_owned()]), details(vec![("label", v("Names")), ("help", v("Who"))])),
            ("odd".into(), "colour".into(), v(1i64), details(vec![])),
        ]);
        assert_eq!(s.len(), 4, "unknown types left out");
        assert_eq!((s[0].kind, &s[0].choices[..]), (Kind::Enum, &["fast".to_owned(), "slow".to_owned()][..]));
        assert_eq!((s[1].range(), s[1].step()), ((1.0, 9.0), 1.0));
        assert_eq!((s[2].label.as_str(), s[2].step()), ("level", 0.1));
        assert_eq!(s[3].help, "Who");
        let wide = Setting { kind: Kind::Int, min: None, max: None, ..s[1].clone() };
        assert_eq!(wide.range(), (-2147483648.0, 2147483647.0));
        assert_eq!(split_list(" a, b ,, c "), ["a", "b", "c"]);
        assert!(split_list(" , ").is_empty());
    }

    #[test]
    fn shortcuts() {
        assert!(shortcut_allowed("t", true));
        assert!(shortcut_allowed("F5", false));
        assert!(!shortcut_allowed("t", false));
        assert!(!shortcut_allowed("Control_L", true));
        assert!(!shortcut_allowed("Fa", false));
    }

    #[test]
    fn permission_texts() {
        assert_eq!(permission_text("files:home"), "read and change all your files");
        assert_eq!(permission_text("something-new"), "something-new");
    }
}
