//! Agent Access (helixdesktop/spec `contracts/agent-access/`): the
//! settings' levels, the shell's `org.helixdesktop.AgentAccess1`
//! interface (the agents it knows; it can only take access away) and the
//! activity log the shell keeps. Blocking zbus; worker threads only.

use std::collections::HashMap;
use std::path::Path;

use helix_app_common::i18n::n_;
use zbus::blocking::Connection;
use zbus::zvariant::OwnedValue;

use crate::bus::{self, Obj, Result};

/// The settings (`contracts/gsettings/org.helixdesktop.agent-access.md`).
pub const SCHEMA: &str = "org.helixdesktop.agent-access";
/// The shell's main connection.
pub const SHELL: &str = "org.gnome.Shell";
const IFACE: &str = "org.helixdesktop.AgentAccess1";
const ACCESS: Obj<'static> = Obj::new(SHELL, "/org/helixdesktop/Agent1", IFACE);

/// `access-level` values, each including the ones before it.
pub const LEVELS: [&str; 4] = ["read-only", "desktop", "apps", "screen-and-input"];
/// The level offered only behind a warning.
pub const SCREEN: &str = "screen-and-input";

/// The Access Level combo's choice (index into the first three levels)
/// and whether the separate Screen and Input switch is on, for a stored
/// `access-level`. Unknown values read as `read-only` (the contract).
pub fn level_choice(level: &str) -> (u32, bool) {
    match level {
        "desktop" => (1, false),
        "apps" => (2, false),
        SCREEN => (2, true),
        _ => (0, false),
    }
}

/// The `access-level` to store for a combo choice and the switch.
pub fn level_value(choice: u32, screen: bool) -> &'static str {
    if screen {
        return SCREEN;
    }
    LEVELS[(choice as usize).min(2)]
}

/// A level's name, untranslated (`None` for a value this version does
/// not know).
pub fn level_label(level: &str) -> Option<&'static str> {
    Some(match level {
        "read-only" => n_("Read Only"),
        "desktop" => n_("Control the Desktop"),
        "apps" => n_("Control Apps"),
        SCREEN => n_("Screen and Input"),
        _ => return None,
    })
}

/// One agent of `ListAgents` (`contracts/agent-access/README.md`, "Who
/// may manage grants").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Agent {
    /// The id the shell uses (executable, or interpreter and script).
    pub id: String,
    pub executable: String,
    /// Self-declared display name; never identity.
    pub name: String,
    /// The level of its grant.
    pub level: String,
    /// Allowed for this session.
    pub session: bool,
    /// Remembered ("Always allow").
    pub always: bool,
    /// Denied this session.
    pub denied: bool,
    /// Unix seconds of its last allowed call this session, 0 for none.
    pub last_active: i64,
    pub identified: bool,
}

fn text(record: &HashMap<String, OwnedValue>, key: &str) -> String {
    bus::get::<String>(record, key).unwrap_or_default()
}

impl Agent {
    /// From a record; `None` without an id. Keys this version does not
    /// know are ignored.
    pub fn from_record(record: &HashMap<String, OwnedValue>) -> Option<Agent> {
        let id = text(record, "agent");
        if id.is_empty() {
            return None;
        }
        let kinds: Vec<String> = bus::get(record, "kinds").unwrap_or_default();
        let has = |k: &str| kinds.iter().any(|x| x == k);
        Some(Agent {
            executable: text(record, "executable"),
            name: text(record, "name"),
            level: text(record, "level"),
            session: has("session"),
            always: has("always"),
            denied: has("denied"),
            last_active: bus::get::<i64>(record, "last-active").unwrap_or(0),
            identified: bus::get::<bool>(record, "identified").unwrap_or(false),
            id,
        })
    }

    /// The row's title: the display name, else the program.
    pub fn title(&self) -> &str {
        if !self.name.is_empty() {
            &self.name
        } else {
            self.program()
        }
    }

    /// The real program (its id, which names the script for an
    /// interpreter), shown next to a display name since a name proves
    /// nothing.
    pub fn program(&self) -> &str {
        if self.id.is_empty() {
            &self.executable
        } else {
            &self.id
        }
    }

    /// Revoke has something to end: a session grant or a denial.
    pub fn can_revoke(&self) -> bool {
        self.session || self.denied
    }

    /// Forget has a remembered grant to forget.
    pub fn can_forget(&self) -> bool {
        self.always
    }
}

/// Most recently active first, then by title.
pub fn sort(agents: &mut [Agent]) {
    agents.sort_by(|a, b| {
        b.last_active
            .cmp(&a.last_active)
            .then_with(|| a.title().to_lowercase().cmp(&b.title().to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });
}

/// Every agent the shell knows, sorted.
pub fn list(conn: &Connection) -> Result<Vec<Agent>> {
    let records: Vec<HashMap<String, OwnedValue>> = ACCESS.call(conn, "ListAgents", &())?;
    let mut agents: Vec<Agent> = records.iter().filter_map(Agent::from_record).collect();
    sort(&mut agents);
    Ok(agents)
}

/// Ends the agent's session grant and denial.
pub fn revoke(conn: &Connection, agent: &str) -> Result<()> {
    ACCESS.run(conn, "Revoke", &(agent,))
}

/// Revoke, and forget its remembered grant.
pub fn forget(conn: &Connection, agent: &str) -> Result<()> {
    ACCESS.run(conn, "Forget", &(agent,))
}

/// Revoke for every agent.
pub fn stop_all(conn: &Connection) -> Result<()> {
    ACCESS.run(conn, "StopAll", &())
}

/// Empties the activity log.
pub fn clear_log(conn: &Connection) -> Result<()> {
    ACCESS.run(conn, "ClearLog", &())
}

/// Calls `changed` for every `AgentsChanged` and whenever the shell
/// comes or goes; returns when `changed` returns false or the bus goes
/// away.
pub fn watch(conn: &Connection, mut changed: impl FnMut() -> bool) {
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(conn) else {
        return;
    };
    let rules = [
        zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender(SHELL)
            .and_then(|b| b.interface(IFACE))
            .and_then(|b| b.member("AgentsChanged"))
            .map(|b| b.build()),
        zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("org.freedesktop.DBus")
            .and_then(|b| b.member("NameOwnerChanged"))
            .and_then(|b| b.arg(0, SHELL))
            .map(|b| b.build()),
    ];
    for rule in rules.into_iter().flatten() {
        let _ = dbus.add_match_rule(rule);
    }
    for message in zbus::blocking::MessageIterator::from(conn) {
        let Ok(message) = message else { continue };
        let header = message.header();
        let wanted = message.message_type() == zbus::message::Type::Signal
            && match header.member().map(|m| m.as_str()) {
                Some("AgentsChanged") => header.interface().is_some_and(|i| i.as_str() == IFACE),
                Some("NameOwnerChanged") => true,
                _ => false,
            };
        if wanted && !changed() {
            return;
        }
    }
}

/// One line of the activity log (`$XDG_STATE_HOME/helix/agent-activity/
/// YYYY-MM-DD.jsonl`). The contract names what a line records; the key
/// names are those `helix-agent log` reads. Missing keys are empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entry {
    /// Unix seconds (0 when unreadable).
    pub time: i64,
    pub agent: String,
    pub name: String,
    pub method: String,
    pub target: String,
    pub level: String,
    /// `allowed`, `refused:<error>` or `cancelled`.
    pub outcome: String,
}

impl Entry {
    fn from_json(v: &serde_json::Value) -> Option<Entry> {
        let obj = v.as_object()?;
        let s = |k: &str| {
            obj.get(k)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let time = match obj.get("time") {
            Some(serde_json::Value::Number(n)) => n.as_i64().unwrap_or(0),
            Some(serde_json::Value::String(t)) => gtk::glib::DateTime::from_iso8601(t, None)
                .map(|d| d.to_unix())
                .unwrap_or(0),
            _ => 0,
        };
        Some(Entry {
            time,
            agent: s("agent"),
            name: s("name"),
            method: s("method"),
            target: s("target"),
            level: s("level"),
            outcome: s("outcome"),
        })
    }

    /// Who: the display name, else the agent id.
    pub fn who(&self) -> &str {
        if self.name.is_empty() {
            &self.agent
        } else {
            &self.name
        }
    }
}

/// How an outcome is described: (untranslated label, error token for
/// refusals).
pub fn outcome_label(outcome: &str) -> (&'static str, Option<&str>) {
    match outcome.split_once(':') {
        Some(("refused", why)) => (n_("Refused"), Some(why)),
        _ if outcome == "refused" => (n_("Refused"), None),
        _ if outcome == "allowed" => (n_("Allowed"), None),
        _ if outcome == "cancelled" => (n_("Cancelled"), None),
        _ => (n_("Unknown"), None),
    }
}

/// The newest `limit` entries of the log in `dir`, newest first, and
/// whether there were more. A missing directory is an empty log;
/// unreadable lines are skipped.
pub fn read_log(dir: &Path, limit: usize) -> (Vec<Entry>, bool) {
    let mut days: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    // YYYY-MM-DD sorts by date.
    days.sort();
    let mut entries = Vec::new();
    let mut more = false;
    for day in days.iter().rev() {
        let Ok(text) = std::fs::read_to_string(day) else {
            continue;
        };
        let mut of_day: Vec<Entry> = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter_map(|v| Entry::from_json(&v))
            .collect();
        of_day.sort_by_key(|e| std::cmp::Reverse(e.time));
        for e in of_day {
            if entries.len() == limit {
                more = true;
                break;
            }
            entries.push(e);
        }
        if more {
            break;
        }
    }
    (entries, more)
}

/// How long ago `then` was, in whole units (`now`, `then` in Unix
/// seconds): `None` for never (0), else (count, unit) with unit `s`
/// (under a minute; count 0), `m`, `h` or `d`.
pub fn ago(now: i64, then: i64) -> Option<(i64, char)> {
    if then <= 0 {
        return None;
    }
    let d = (now - then).max(0);
    Some(match d {
        0..=59 => (0, 's'),
        60..=3599 => (d / 60, 'm'),
        3600..=86399 => (d / 3600, 'h'),
        _ => (d / 86400, 'd'),
    })
}

/// `excluded-apps` with `app` added (without a `.desktop` suffix, as the
/// contract compares them), or `None` when it is already there.
pub fn with_excluded(list: &[String], app: &str) -> Option<Vec<String>> {
    let app = app.strip_suffix(".desktop").unwrap_or(app);
    if app.is_empty()
        || list
            .iter()
            .any(|a| a.strip_suffix(".desktop").unwrap_or(a) == app)
    {
        return None;
    }
    let mut out = list.to_vec();
    out.push(app.to_string());
    Some(out)
}

/// `excluded-apps` without `app`.
pub fn without_excluded(list: &[String], app: &str) -> Vec<String> {
    list.iter()
        .filter(|a| a.strip_suffix(".desktop").unwrap_or(a) != app)
        .cloned()
        .collect()
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::bus::testbus;
    use std::sync::{Arc, Mutex};
    use zbus::object_server::SignalEmitter;
    use zbus::zvariant::Value;

    #[test]
    fn levels_map_to_the_combo_and_the_switch() {
        assert_eq!(level_choice("read-only"), (0, false));
        assert_eq!(level_choice("desktop"), (1, false));
        assert_eq!(level_choice("apps"), (2, false));
        assert_eq!(level_choice("screen-and-input"), (2, true));
        assert_eq!(
            level_choice("nonsense"),
            (0, false),
            "unknown reads as read-only"
        );
        for level in LEVELS {
            let (choice, screen) = level_choice(level);
            assert_eq!(level_value(choice, screen), level, "round trip");
            assert!(level_label(level).is_some());
        }
        // Turning Screen and Input off leaves Control Apps.
        assert_eq!(level_value(2, false), "apps");
        assert_eq!(level_value(0, true), SCREEN);
        assert_eq!(level_value(9, false), "apps");
        assert_eq!(level_label("x"), None);
    }

    fn record(pairs: &[(&str, Value<'_>)]) -> HashMap<String, OwnedValue> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.try_to_owned().unwrap()))
            .collect()
    }

    #[test]
    fn agents_from_records() {
        let a = Agent::from_record(&record(&[
            ("agent", Value::from("/usr/bin/node /home/me/agent.js")),
            ("executable", Value::from("/usr/bin/node")),
            ("name", Value::from("Example Agent")),
            ("level", Value::from("desktop")),
            ("kinds", Value::from(vec!["session", "always"])),
            ("granted", Value::from(1795132800i64)),
            ("last-active", Value::from(1795132900i64)),
            ("identified", Value::from(true)),
            ("future-key", Value::from(7u32)),
        ]))
        .unwrap();
        assert_eq!(a.title(), "Example Agent");
        assert_eq!(a.program(), "/usr/bin/node /home/me/agent.js");
        assert!(a.session && a.always && !a.denied && a.identified);
        assert!(a.can_revoke() && a.can_forget());
        assert_eq!(a.last_active, 1795132900);

        let b = Agent::from_record(&record(&[
            ("agent", Value::from("/usr/bin/foot")),
            ("kinds", Value::from(vec!["denied"])),
        ]))
        .unwrap();
        assert_eq!(b.title(), "/usr/bin/foot", "no name: the program");
        assert!(b.can_revoke() && !b.can_forget());
        assert_eq!(b.last_active, 0);

        let c = Agent::from_record(&record(&[("kinds", Value::from(vec!["always"]))]));
        assert_eq!(c, None, "no id");

        let mut list = vec![
            Agent {
                id: "/b".into(),
                name: "beta".into(),
                ..Agent::default()
            },
            Agent {
                id: "/a".into(),
                name: "Alpha".into(),
                ..Agent::default()
            },
            Agent {
                id: "/c".into(),
                last_active: 5,
                ..Agent::default()
            },
        ];
        sort(&mut list);
        let ids: Vec<&str> = list.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["/c", "/a", "/b"]);
    }

    #[test]
    fn outcomes_and_times() {
        assert_eq!(outcome_label("allowed"), ("Allowed", None));
        assert_eq!(
            outcome_label("refused:LevelTooLow"),
            ("Refused", Some("LevelTooLow"))
        );
        assert_eq!(outcome_label("cancelled"), ("Cancelled", None));
        assert_eq!(outcome_label("odd"), ("Unknown", None));
        assert_eq!(ago(100, 0), None);
        assert_eq!(ago(100, 90), Some((0, 's')));
        assert_eq!(ago(1000, 100), Some((15, 'm')));
        assert_eq!(ago(10_000, 100), Some((2, 'h')));
        assert_eq!(ago(300_000, 100), Some((3, 'd')));
        assert_eq!(ago(100, 200), Some((0, 's')), "clock skew");
    }

    #[test]
    fn excluded_apps_list() {
        let list = vec!["org.example.Bank".to_string()];
        assert_eq!(with_excluded(&list, "org.example.Bank.desktop"), None);
        assert_eq!(with_excluded(&list, ""), None);
        assert_eq!(
            with_excluded(&list, "org.gnome.Maps.desktop").unwrap(),
            ["org.example.Bank", "org.gnome.Maps"]
        );
        let dotted = vec!["org.example.Bank.desktop".to_string()];
        assert_eq!(with_excluded(&dotted, "org.example.Bank"), None);
        assert!(without_excluded(&dotted, "org.example.Bank").is_empty());
        assert_eq!(without_excluded(&list, "x"), list);
    }

    #[test]
    fn log_is_read_newest_first() {
        let dir = std::env::temp_dir().join(format!("helix-agent-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(read_log(&dir, 10), (Vec::new(), false), "no directory");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("2026-11-18.jsonl"),
            "{\"time\":1795000000,\"agent\":\"/usr/bin/a\",\"name\":\"A\",\"method\":\"GetState\",\"outcome\":\"allowed\"}\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("2026-11-19.jsonl"),
            "{\"time\":1795082400,\"agent\":\"/usr/bin/a\",\"name\":\"A\",\"method\":\"ListWindows\",\"level\":\"read-only\",\"outcome\":\"allowed\"}\n\
             not json\n\
             \n\
             {\"time\":\"2026-11-19T10:01:40Z\",\"agent\":\"/usr/bin/b\",\"method\":\"Open\",\"target\":\"file:///tmp/x\",\"outcome\":\"refused:LevelTooLow\"}\n",
        )
        .unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();
        let (all, more) = read_log(&dir, 10);
        assert!(!more);
        let methods: Vec<&str> = all.iter().map(|e| e.method.as_str()).collect();
        assert_eq!(methods, ["Open", "ListWindows", "GetState"]);
        assert_eq!(all[0].time, 1795082500);
        assert_eq!(all[0].who(), "/usr/bin/b");
        assert_eq!(all[0].target, "file:///tmp/x");
        assert_eq!(all[1].who(), "A");
        assert_eq!(all[1].level, "read-only");
        let (two, more) = read_log(&dir, 2);
        assert_eq!(two.len(), 2);
        assert!(more);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The shell's side, as the contract describes it (without its
    /// caller check).
    pub struct FakeShell {
        pub agents: Arc<Mutex<Vec<Agent>>>,
        pub calls: Arc<Mutex<Vec<String>>>,
    }

    fn to_record(a: &Agent) -> HashMap<String, OwnedValue> {
        let mut kinds = Vec::new();
        for (on, k) in [
            (a.session, "session"),
            (a.always, "always"),
            (a.denied, "denied"),
        ] {
            if on {
                kinds.push(k);
            }
        }
        record(&[
            ("agent", Value::from(a.id.as_str())),
            ("executable", Value::from(a.executable.as_str())),
            ("name", Value::from(a.name.as_str())),
            ("level", Value::from(a.level.as_str())),
            ("kinds", Value::from(kinds)),
            ("granted", Value::from(1795000000i64)),
            ("last-active", Value::from(a.last_active)),
            ("identified", Value::from(a.identified)),
        ])
    }

    #[zbus::interface(name = "org.helixdesktop.AgentAccess1")]
    impl FakeShell {
        fn list_agents(&self) -> Vec<HashMap<String, OwnedValue>> {
            self.agents.lock().unwrap().iter().map(to_record).collect()
        }
        async fn revoke(&self, agent: String, #[zbus(signal_emitter)] e: SignalEmitter<'_>) {
            self.calls.lock().unwrap().push(format!("Revoke {agent}"));
            self.agents.lock().unwrap().retain_mut(|a| {
                if a.id == agent {
                    a.session = false;
                    a.denied = false;
                }
                a.always || a.session || a.denied
            });
            let _ = Self::agents_changed(&e).await;
        }
        async fn forget(&self, agent: String, #[zbus(signal_emitter)] e: SignalEmitter<'_>) {
            self.calls.lock().unwrap().push(format!("Forget {agent}"));
            self.agents.lock().unwrap().retain(|a| a.id != agent);
            let _ = Self::agents_changed(&e).await;
        }
        async fn stop_all(&self, #[zbus(signal_emitter)] e: SignalEmitter<'_>) {
            self.calls.lock().unwrap().push("StopAll".into());
            self.agents.lock().unwrap().retain_mut(|a| {
                a.session = false;
                a.denied = false;
                a.always
            });
            let _ = Self::agents_changed(&e).await;
        }
        fn clear_log(&self) {
            self.calls.lock().unwrap().push("ClearLog".into());
        }
        #[zbus(signal)]
        async fn agents_changed(e: &SignalEmitter<'_>) -> zbus::Result<()>;
    }

    /// Example agents for the tests and for looking at the page.
    pub fn sample_agents() -> Vec<Agent> {
        vec![
            Agent {
                id: "/usr/bin/node /home/me/.local/lib/assistant/cli.js".into(),
                executable: "/usr/bin/node".into(),
                name: "Coding Assistant".into(),
                level: "desktop".into(),
                session: true,
                always: true,
                last_active: 1795082400,
                identified: true,
                ..Agent::default()
            },
            Agent {
                id: "/usr/bin/foot".into(),
                executable: "/usr/bin/foot".into(),
                level: "read-only".into(),
                denied: true,
                ..Agent::default()
            },
        ]
    }

    pub fn serve(conn: &Connection, agents: Vec<Agent>) -> Arc<Mutex<Vec<String>>> {
        let calls = serve_object(conn, agents);
        conn.request_name(SHELL).unwrap();
        calls
    }

    /// The object only, on a connection that already owns [`SHELL`].
    pub fn serve_object(conn: &Connection, agents: Vec<Agent>) -> Arc<Mutex<Vec<String>>> {
        let calls = Arc::new(Mutex::new(Vec::new()));
        conn.object_server()
            .at(
                "/org/helixdesktop/Agent1",
                FakeShell {
                    agents: Arc::new(Mutex::new(agents)),
                    calls: calls.clone(),
                },
            )
            .unwrap();
        calls
    }

    #[test]
    fn against_a_fake_shell() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let Some(bus) = testbus::private_bus() else {
            return;
        };
        let app = testbus::connect(&bus);
        assert_eq!(list(&app), Err(bus::Error::Missing), "no shell");

        let changes = Arc::new(AtomicUsize::new(0));
        let c = changes.clone();
        let watcher = testbus::connect(&bus);
        std::thread::spawn(move || {
            watch(&watcher, || {
                c.fetch_add(1, Ordering::SeqCst);
                true
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(100));

        let service = testbus::connect(&bus);
        let calls = serve(&service, sample_agents());
        assert!(
            testbus::eventually(|| changes.load(Ordering::SeqCst) >= 1),
            "the shell came"
        );
        let agents = list(&app).unwrap();
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].title(), "Coding Assistant", "active first");
        assert!(agents[1].denied);

        let before = changes.load(Ordering::SeqCst);
        revoke(&app, "/usr/bin/foot").unwrap();
        assert!(testbus::eventually(
            || changes.load(Ordering::SeqCst) > before
        ));
        assert_eq!(list(&app).unwrap().len(), 1);

        // A remembered grant outlives Revoke and StopAll, not Forget.
        stop_all(&app).unwrap();
        let left = list(&app).unwrap();
        assert!(left[0].always && !left[0].session);
        forget(&app, &left[0].id).unwrap();
        assert!(list(&app).unwrap().is_empty());
        // Unknown agents are not an error.
        revoke(&app, "/nowhere").unwrap();
        clear_log(&app).unwrap();
        assert_eq!(
            *calls.lock().unwrap(),
            [
                "Revoke /usr/bin/foot",
                "StopAll",
                "Forget /usr/bin/node /home/me/.local/lib/assistant/cli.js",
                "Revoke /nowhere",
                "ClearLog"
            ]
        );
    }
}
