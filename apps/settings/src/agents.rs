//! Agent Access (helixdesktop/spec `contracts/agent-access/`): the
//! settings' levels, the shell's `org.helixdesktop.AgentAccess1`
//! interface (the agents it knows; it can only take access away) and the
//! activity log the shell keeps. Blocking zbus; worker threads only.

use std::collections::HashMap;
use std::path::Path;

use helix_app_common::i18n::{n_, tr, tr_dyn};
use helix_app_common::{trf, trnf};
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
/// YYYY-MM-DD.jsonl`, contract "Activity log"). Missing keys are empty
/// (0); keys this version does not know are ignored, and so are lengths
/// (`query-length` and the like), which say little to the user.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entry {
    /// Unix seconds (0 when unreadable).
    pub time: i64,
    pub agent: String,
    pub name: String,
    /// An `Agent1` method, an `AgentAccess1` one or an event.
    pub method: String,
    pub target: String,
    pub level: String,
    /// `allowed`, `refused:<error>` or `cancelled`.
    pub outcome: String,
    /// An allowed call that then failed: the error's short name.
    pub error: String,
    /// Consent prompts: `session` or `always`.
    pub grant: String,
    /// Coalesced refusals: how many calls the line stands for (0 or 1
    /// for one) and the last one's time.
    pub count: u64,
    pub until: i64,
    /// `LogFull`: `refusals` or `all`.
    pub dropped: String,
}

/// A time: Unix seconds, or ISO 8601 text (0 when unreadable).
fn json_time(v: Option<&serde_json::Value>) -> i64 {
    match v {
        Some(serde_json::Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(serde_json::Value::String(t)) => gtk::glib::DateTime::from_iso8601(t, None)
            .map(|d| d.to_unix())
            .unwrap_or(0),
        _ => 0,
    }
}

impl Entry {
    /// From one line; `None` for one that is not a JSON object.
    pub fn parse(line: &str) -> Option<Entry> {
        let v = serde_json::from_str::<serde_json::Value>(line.trim()).ok()?;
        let obj = v.as_object()?;
        let s = |k: &str| {
            obj.get(k)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        };
        Some(Entry {
            time: json_time(obj.get("time")),
            agent: s("agent"),
            name: s("name"),
            method: s("method"),
            target: s("target"),
            level: s("level"),
            outcome: s("outcome"),
            error: s("error"),
            grant: s("grant"),
            count: obj.get("count").and_then(|v| v.as_u64()).unwrap_or(0),
            until: json_time(obj.get("until")),
            dropped: s("dropped"),
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

    /// The refusal's error name for `refused:<Error>`.
    pub fn refusal(&self) -> Option<&str> {
        match self.outcome.split_once(':') {
            Some(("refused", why)) => Some(why),
            _ if self.outcome == "refused" => Some(""),
            _ => None,
        }
    }

    /// The last call a coalesced line stands for, when it is one.
    pub fn coalesced(&self) -> Option<(u64, i64)> {
        (self.count >= 2).then_some((self.count, self.until.max(self.time)))
    }
}

/// The `AgentAccess1` methods (Settings' own; `target` is the agent).
const SETTINGS_METHODS: [&str; 5] = ["ListAgents", "Revoke", "Forget", "StopAll", "ClearLog"];

/// What a method or event did, as a short untranslated phrase (`None`
/// for a name this version does not know). Every method of `Agent1` and
/// `AgentAccess1`, and the log's events.
pub fn method_label(method: &str) -> Option<&'static str> {
    Some(match method {
        // Agent1.
        "GetStatus" => n_("Checked its access"),
        "RequestAccess" => n_("Asked for access"),
        "GetState" => n_("Read the desktop state"),
        "ListWindows" => n_("Listed windows"),
        "ListApps" => n_("Listed apps"),
        "ListWorkspaces" => n_("Listed workspaces"),
        "ListMonitors" => n_("Listed displays"),
        "ListNotifications" => n_("Listed notifications"),
        "RecentFiles" => n_("Listed recent files"),
        "SearchFiles" => n_("Searched files"),
        "WaitForWindow" => n_("Waited for a window"),
        "WaitForWindowClosed" => n_("Waited for a window to close"),
        "Ask" => n_("Asked you a question"),
        "Confirm" => n_("Asked you to confirm"),
        "AddReminder" => n_("Added a reminder"),
        "ListReminders" => n_("Listed reminders"),
        "CancelReminder" => n_("Cancelled a reminder"),
        "Open" => n_("Opened a file or link"),
        "Launch" => n_("Started an app"),
        "WindowAction" => n_("Changed a window"),
        "SwitchWorkspace" => n_("Switched workspace"),
        "SetToggle" => n_("Turned a setting on or off"),
        "SetValue" => n_("Adjusted a setting"),
        "Notify" => n_("Sent a notification"),
        "GetClipboard" => n_("Read the clipboard"),
        "SetClipboard" => n_("Changed the clipboard"),
        "A11yTree" => n_("Read an app’s contents"),
        "A11yAction" => n_("Used a control in an app"),
        "A11ySetValue" => n_("Filled in a field in an app"),
        "Screenshot" => n_("Took a screenshot"),
        "Input" => n_("Typed or clicked"),
        // AgentAccess1, as Settings did it.
        "ListAgents" => n_("Listed agents"),
        "Revoke" => n_("Access revoked"),
        "Forget" => n_("Access revoked and forgotten"),
        "StopAll" => n_("All access revoked"),
        "ClearLog" => n_("Activity log cleared"),
        // Events.
        "ConsentPrompt" => n_("Asked for access"),
        "SensitivePrompt" => n_("Asked to confirm an action"),
        "Stop" => n_("Stopped by you"),
        "LogFull" => n_("Activity log full"),
        _ => return None,
    })
}

/// An `AgentAccess1` call that was refused (a program other than
/// Settings tried it).
pub fn settings_method_refused_label(method: &str) -> Option<&'static str> {
    Some(match method {
        "ListAgents" => n_("Tried to list agents"),
        "Revoke" => n_("Tried to revoke access"),
        "Forget" => n_("Tried to forget an agent"),
        "StopAll" => n_("Tried to revoke all access"),
        "ClearLog" => n_("Tried to clear the activity log"),
        _ => return None,
    })
}

/// Why the policy refused a call (`refused:<Error>`), untranslated.
pub fn refusal_label(error: &str) -> Option<&'static str> {
    Some(match error {
        "Sandboxed" => n_("From a sandboxed app"),
        "AccessOff" => n_("Agent Access was off"),
        "Remote" => n_("From a remote session"),
        "Locked" => n_("The screen was locked"),
        "LevelTooLow" => n_("Needs a higher access level"),
        "FeatureOff" => n_("That feature was turned off"),
        "NotGranted" => n_("Not allowed yet"),
        "DeniedByUser" => n_("Denied by you"),
        "Excluded" => n_("Excluded app"),
        "NeedsConfirmation" => n_("Needed your confirmation"),
        "AccessDenied" => n_("Only Settings may do this"),
        _ => return None,
    })
}

/// How an allowed call then failed (`error`), untranslated.
pub fn failure_label(error: &str) -> Option<&'static str> {
    Some(match error {
        "TimedOut" => n_("Failed: timed out"),
        "NotFound" => n_("Failed: not found"),
        "NotSupported" => n_("Failed: not supported"),
        "InvalidArgs" => n_("Failed: invalid request"),
        "Cancelled" => n_("Failed: cancelled"),
        "Failed" => n_("Failed"),
        _ => return None,
    })
}

/// A log line as the page shows it, translated: what happened, what it
/// was done to, who did it, and how it went.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Described {
    pub title: String,
    /// Empty for none.
    pub target: String,
    /// Empty for none (`LogFull`).
    pub who: String,
    pub status: Vec<String>,
}

/// A target for people: a window by its app, a `file://` URI as a path;
/// a `SensitivePrompt`'s method by what it does.
pub fn target_label(method: &str, target: &str) -> String {
    if method == "SensitivePrompt" {
        return method_label(target).map_or_else(|| target.to_string(), tr_dyn);
    }
    if let Some(window) = target.strip_prefix("window:") {
        return match window.split_once(" (") {
            Some((_, app)) if app.ends_with(')') && app.len() > 1 => {
                trf!("A window of {app}", app = &app[..app.len() - 1])
            }
            _ => tr("A window").to_string(),
        };
    }
    if target.starts_with("file://") {
        if let Ok((path, _)) = gtk::glib::filename_from_uri(target) {
            return path.display().to_string();
        }
    }
    target.to_string()
}

/// Describes `e` (contract "Activity log": lines, outcomes and events).
pub fn describe(e: &Entry) -> Described {
    let method = e.method.as_str();
    let raw = |s: &str| s.to_string();
    let settings = SETTINGS_METHODS.contains(&method);
    let refusal = e.refusal();
    let title = match (settings && refusal.is_some())
        .then(|| settings_method_refused_label(method))
        .flatten()
        .or_else(|| method_label(method))
    {
        Some(t) => tr_dyn(t),
        None => raw(method),
    };
    let who = if settings && e.outcome == "allowed" {
        tr("Settings").to_string()
    } else {
        raw(e.who())
    };
    let reason = |why: &str| match refusal_label(why) {
        Some(t) => tr_dyn(t),
        None => raw(why),
    };
    let mut status: Vec<String> = Vec::new();
    match (method, e.outcome.as_str()) {
        ("ConsentPrompt", outcome) => {
            if let Some(level) = level_label(&e.level) {
                status.push(tr_dyn(level));
            }
            status.push(match (outcome, e.grant.as_str()) {
                ("allowed", "session") => tr("Allowed for this session").to_string(),
                ("allowed", "always") => tr("Always allowed").to_string(),
                ("allowed", _) => tr("Allowed").to_string(),
                ("refused:DeniedByUser", _) => tr("Denied").to_string(),
                ("cancelled", _) => tr("Not answered").to_string(),
                _ => raw(outcome),
            });
        }
        ("SensitivePrompt", outcome) => status.push(match outcome {
            "allowed" => tr("Allowed").to_string(),
            "refused:DeniedByUser" => tr("Denied").to_string(),
            "cancelled" => tr("Not answered").to_string(),
            _ => raw(outcome),
        }),
        // The title says it all.
        ("Stop", "cancelled") => {}
        ("LogFull", _) => match e.dropped.as_str() {
            "refusals" => status.push(tr("Refused calls are no longer recorded today").to_string()),
            "all" => status.push(tr("Nothing more is recorded today").to_string()),
            _ => {}
        },
        (_, "allowed") if settings => {}
        (_, "allowed") => {
            status.push(tr("Allowed").to_string());
            if !e.error.is_empty() {
                status.push(match failure_label(&e.error) {
                    Some(t) => tr_dyn(t),
                    None => trf!("Failed: {error}", error = e.error),
                });
            }
        }
        (_, "cancelled") => status.push(tr("Cancelled").to_string()),
        (_, outcome) => match refusal {
            Some(why) => {
                status.push(match e.coalesced() {
                    Some((n, _)) => trnf!("Refused {n} time", "Refused {n} times", n),
                    None => tr("Refused").to_string(),
                });
                if !why.is_empty() {
                    status.push(reason(why));
                }
            }
            None => status.push(raw(outcome)),
        },
    }
    Described {
        title,
        target: target_label(method, &e.target),
        who,
        status,
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
            .filter_map(Entry::parse)
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

    /// Every method of the contract's interfaces
    /// (`contracts/agent-access/*.xml`) and the log's events.
    const METHODS: [&str; 36] = [
        "GetStatus",
        "RequestAccess",
        "GetState",
        "ListWindows",
        "ListApps",
        "ListWorkspaces",
        "ListMonitors",
        "ListNotifications",
        "RecentFiles",
        "SearchFiles",
        "WaitForWindow",
        "WaitForWindowClosed",
        "Ask",
        "Confirm",
        "AddReminder",
        "ListReminders",
        "CancelReminder",
        "Open",
        "Launch",
        "WindowAction",
        "SwitchWorkspace",
        "SetToggle",
        "SetValue",
        "Notify",
        "GetClipboard",
        "SetClipboard",
        "A11yTree",
        "A11yAction",
        "A11ySetValue",
        "Screenshot",
        "Input",
        "ListAgents",
        "Revoke",
        "Forget",
        "StopAll",
        "ClearLog",
    ];
    const EVENTS: [&str; 4] = ["ConsentPrompt", "SensitivePrompt", "Stop", "LogFull"];

    #[test]
    fn every_method_and_event_has_a_label() {
        for m in METHODS.iter().chain(&EVENTS) {
            let label = method_label(m).unwrap_or_else(|| panic!("{m}"));
            assert_ne!(label, *m, "{m}: a phrase, not the name");
        }
        // The contract's XML, when it is next to this repository.
        let spec = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../helix-desktop-spec/contracts/agent-access");
        for file in [
            "org.helixdesktop.Agent1.xml",
            "org.helixdesktop.AgentAccess1.xml",
        ] {
            let Ok(xml) = std::fs::read_to_string(spec.join(file)) else {
                continue;
            };
            for part in xml.split("<method name=\"").skip(1) {
                let name = part.split('"').next().unwrap();
                assert!(METHODS.contains(&name), "{name} from {file}");
            }
        }
        for m in SETTINGS_METHODS {
            assert!(settings_method_refused_label(m).is_some(), "{m}");
        }
        assert_eq!(method_label("FutureMethod"), None);
        assert_eq!(method_label("ListWindows"), Some("Listed windows"));
        assert_eq!(method_label("SearchFiles"), Some("Searched files"));
        assert_eq!(method_label("Stop"), Some("Stopped by you"));
        assert_eq!(method_label("Revoke"), Some("Access revoked"));
        for why in [
            "Sandboxed",
            "AccessOff",
            "Remote",
            "Locked",
            "LevelTooLow",
            "FeatureOff",
            "NotGranted",
            "DeniedByUser",
            "Excluded",
            "NeedsConfirmation",
            "AccessDenied",
        ] {
            assert!(refusal_label(why).is_some(), "{why}");
        }
        assert_eq!(refusal_label("Later"), None);
        for error in [
            "TimedOut",
            "NotFound",
            "NotSupported",
            "Failed",
            "InvalidArgs",
        ] {
            assert!(failure_label(error).is_some(), "{error}");
        }
        assert_eq!(failure_label("TimedOut"), Some("Failed: timed out"));
        assert_eq!(failure_label("Later"), None);
    }

    fn shown(line: &str) -> (String, String, String, String) {
        let d = describe(&Entry::parse(line).expect(line));
        (d.title, d.target, d.who, d.status.join(" · "))
    }

    fn four(a: &str, b: &str, c: &str, d: &str) -> (String, String, String, String) {
        (a.into(), b.into(), c.into(), d.into())
    }

    #[test]
    fn the_contracts_example_lines() {
        let e = Entry::parse(r#"{"time":1795082400,"agent":"/usr/bin/node /home/me/agent.js","name":"Example","method":"ListWindows","target":"org.gnome.TextEditor","level":"read-only","outcome":"allowed"}"#).unwrap();
        assert_eq!(
            (e.time, e.level.as_str(), e.count),
            (1795082400, "read-only", 0)
        );
        assert_eq!(e.coalesced(), None);
        assert_eq!(
            describe(&e),
            Described {
                title: "Listed windows".into(),
                target: "org.gnome.TextEditor".into(),
                who: "Example".into(),
                status: vec!["Allowed".into()],
            }
        );
        // Lengths are not shown.
        assert_eq!(
            shown(
                r#"{"time":1795082410,"agent":"/usr/bin/node /home/me/agent.js","name":"Example","method":"SearchFiles","target":"file:///home/me","level":"read-only","outcome":"allowed","query-length":6}"#
            ),
            four("Searched files", "/home/me", "Example", "Allowed")
        );
        assert_eq!(
            shown(
                r#"{"time":1795082420,"agent":"/usr/bin/node /home/me/agent.js","name":"Example","method":"WaitForWindowClosed","target":"window:21 (org.example.Vault)","level":"read-only","outcome":"refused:Excluded"}"#
            ),
            four(
                "Waited for a window to close",
                "A window of org.example.Vault",
                "Example",
                "Refused · Excluded app"
            )
        );
        assert_eq!(
            shown(
                r#"{"time":1795082430,"agent":"/usr/bin/node /home/me/agent.js","name":"Example","method":"WaitForWindow","level":"read-only","outcome":"allowed","error":"TimedOut","title-length":5}"#
            ),
            four(
                "Waited for a window",
                "",
                "Example",
                "Allowed · Failed: timed out"
            )
        );
        assert_eq!(
            shown(
                r#"{"time":1795082440,"agent":"/usr/bin/node /home/me/agent.js","name":"Example","method":"ConsentPrompt","level":"read-only","outcome":"allowed","grant":"session"}"#
            ),
            four(
                "Asked for access",
                "",
                "Example",
                "Read Only · Allowed for this session"
            )
        );
        let line = r#"{"time":1795082501,"agent":"/opt/bot","name":"","method":"GetState","level":"read-only","outcome":"refused:AccessOff","count":120,"until":1795082559}"#;
        assert_eq!(
            Entry::parse(line).unwrap().coalesced(),
            Some((120, 1795082559))
        );
        assert_eq!(
            shown(line),
            four(
                "Read the desktop state",
                "",
                "/opt/bot",
                "Refused 120 times · Agent Access was off"
            )
        );
    }

    #[test]
    fn other_line_shapes() {
        let line =
            |rest: &str| format!(r#"{{"time":1795082400,"agent":"/opt/bot","name":"Bot",{rest}}}"#);
        // Consent prompts.
        assert_eq!(
            shown(&line(
                r#""method":"ConsentPrompt","level":"desktop","outcome":"allowed","grant":"always""#
            ))
            .3,
            "Control the Desktop · Always allowed"
        );
        assert_eq!(
            shown(&line(
                r#""method":"ConsentPrompt","level":"read-only","outcome":"refused:DeniedByUser""#
            ))
            .3,
            "Read Only · Denied"
        );
        assert_eq!(
            shown(&line(r#""method":"ConsentPrompt","outcome":"cancelled""#)).3,
            "Not answered"
        );
        // Sensitive-action prompts name the method by what it does.
        assert_eq!(
            shown(&line(
                r#""method":"SensitivePrompt","target":"Open","outcome":"refused:DeniedByUser""#
            )),
            four(
                "Asked to confirm an action",
                "Opened a file or link",
                "Bot",
                "Denied"
            )
        );
        assert_eq!(
            shown(&line(
                r#""method":"SensitivePrompt","target":"Later","outcome":"allowed""#
            ))
            .1,
            "Later"
        );
        assert_eq!(
            shown(&line(
                r#""method":"Stop","level":"apps","outcome":"cancelled""#
            )),
            four("Stopped by you", "", "Bot", "")
        );
        assert_eq!(
            shown(
                r#"{"time":1795082400,"agent":"","name":"","method":"LogFull","outcome":"allowed","dropped":"refusals"}"#
            ),
            four(
                "Activity log full",
                "",
                "",
                "Refused calls are no longer recorded today"
            )
        );
        assert_eq!(
            shown(r#"{"time":1795082400,"agent":"","name":"","method":"LogFull","outcome":"allowed","dropped":"all"}"#).3,
            "Nothing more is recorded today"
        );
        // Settings' own calls.
        let settings = r#"{"time":1795082400,"agent":"/usr/bin/helix-settings-app","name":"","#;
        assert_eq!(
            shown(&format!(
                r#"{settings}"method":"ClearLog","outcome":"allowed"}}"#
            )),
            four("Activity log cleared", "", "Settings", "")
        );
        assert_eq!(
            shown(&format!(
                r#"{settings}"method":"Revoke","target":"/opt/bot","outcome":"allowed"}}"#
            )),
            four("Access revoked", "/opt/bot", "Settings", "")
        );
        assert_eq!(
            shown(&format!(
                r#"{settings}"method":"Forget","target":"/opt/bot","outcome":"allowed"}}"#
            ))
            .0,
            "Access revoked and forgotten"
        );
        assert_eq!(
            shown(&format!(
                r#"{settings}"method":"StopAll","outcome":"allowed"}}"#
            ))
            .0,
            "All access revoked"
        );
        // Another program trying.
        assert_eq!(
            shown(&line(
                r#""method":"ClearLog","outcome":"refused:AccessDenied""#
            )),
            four(
                "Tried to clear the activity log",
                "",
                "Bot",
                "Refused · Only Settings may do this"
            )
        );
        // Failures and refusals this version does not know, unknown
        // methods and outcomes: the raw names.
        assert_eq!(
            shown(&line(
                r#""method":"Open","target":"file:///tmp/a%20b","outcome":"allowed","error":"Busy""#
            )),
            four(
                "Opened a file or link",
                "/tmp/a b",
                "Bot",
                "Allowed · Failed: Busy"
            )
        );
        assert_eq!(
            shown(&line(
                r#""method":"Teleport","outcome":"refused:TooFar","count":1"#
            )),
            four("Teleport", "", "Bot", "Refused · TooFar")
        );
        assert_eq!(
            shown(&line(r#""method":"Open","outcome":"refused""#)).3,
            "Refused"
        );
        assert_eq!(
            shown(&line(r#""method":"Open","outcome":"maybe""#)).3,
            "maybe"
        );
        assert_eq!(
            shown(&line(r#""method":"Ask","outcome":"cancelled""#)).3,
            "Cancelled"
        );
        assert_eq!(
            shown(&line(
                r#""method":"WindowAction","target":"window:7","outcome":"allowed""#
            ))
            .1,
            "A window"
        );
        // No name: the agent id.
        assert_eq!(
            shown(r#"{"time":1,"agent":"/usr/bin/foot","method":"ListApps","outcome":"allowed"}"#)
                .2,
            "/usr/bin/foot"
        );
        // Tolerance: other types, unknown keys, ISO times.
        let e = Entry::parse(r#"{"time":"2026-11-19T10:00:00Z","method":7,"count":"x","until":"2026-11-19T10:00:30Z","new":{"a":1}}"#).unwrap();
        assert_eq!((e.time, e.until, e.count), (1795082400, 1795082430, 0));
        assert_eq!(e.method, "");
        assert_eq!(Entry::parse("[1,2]"), None);
        assert_eq!(Entry::parse("{\"time\":1"), None);
        // A coalesced line's end is never before its start.
        let e = Entry::parse(r#"{"time":100,"count":3}"#).unwrap();
        assert_eq!(e.coalesced(), Some((3, 100)));
    }

    #[test]
    fn times() {
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
