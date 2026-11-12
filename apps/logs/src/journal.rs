//! Journal entries as emitted by `journalctl -o json`, boot lists, and the
//! journalctl command line.

use serde_json::Value;

/// Placeholder shown for fields journalctl did not serialise (`null`).
pub const OMITTED: &str = "[data omitted]";

/// One journal entry. Fields keep journalctl's order of appearance only
/// loosely: they are sorted by name so lookups and display are stable.
/// A field that occurs several times appears several times.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Entry {
    fields: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParseError {
    Json(String),
    NotAnObject,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Json(e) => write!(f, "invalid JSON: {e}"),
            ParseError::NotAnObject => write!(f, "journal entry is not a JSON object"),
        }
    }
}

/// Decodes a byte-array value (`[72, 105]`). Returns `None` if `arr` is not
/// a list of numbers 0–255.
fn decode_bytes(arr: &[Value]) -> Option<String> {
    let bytes: Option<Vec<u8>> = arr
        .iter()
        .map(|v| v.as_u64().filter(|n| *n <= 255).map(|n| n as u8))
        .collect();
    bytes.map(|b| String::from_utf8_lossy(&b).into_owned())
}

fn push_value(out: &mut Vec<(String, String)>, key: &str, v: &Value, nested: bool) {
    match v {
        Value::String(s) => out.push((key.to_string(), s.clone())),
        Value::Null => out.push((key.to_string(), OMITTED.to_string())),
        Value::Number(n) => out.push((key.to_string(), n.to_string())),
        Value::Bool(b) => out.push((key.to_string(), b.to_string())),
        Value::Array(arr) => {
            // An empty array is an empty byte string.
            if let Some(s) = decode_bytes(arr) {
                out.push((key.to_string(), s));
            } else if !nested {
                // Multi-valued field: each element is a value of its own.
                for item in arr {
                    push_value(out, key, item, true);
                }
            } else {
                out.push((key.to_string(), v.to_string()));
            }
        }
        Value::Object(_) => out.push((key.to_string(), v.to_string())),
    }
}

impl Entry {
    #[cfg(test)]
    pub fn from_fields<K: Into<String>, V: Into<String>>(fields: impl IntoIterator<Item = (K, V)>) -> Self {
        let mut fields: Vec<(String, String)> = fields.into_iter().map(|(k, v)| (k.into(), v.into())).collect();
        fields.sort_by(|a, b| a.0.cmp(&b.0));
        Entry { fields }
    }

    /// Parses one line of `journalctl -o json` output.
    pub fn parse(line: &str) -> Result<Entry, ParseError> {
        let v: Value = serde_json::from_str(line).map_err(|e| ParseError::Json(e.to_string()))?;
        let Value::Object(map) = v else { return Err(ParseError::NotAnObject) };
        let mut fields = Vec::with_capacity(map.len());
        for (k, v) in &map {
            push_value(&mut fields, k, v, false);
        }
        // Stable sort keeps multiple occurrences in their original order.
        fields.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(Entry { fields })
    }

    /// First value of `key`.
    pub fn get(&self, key: &str) -> Option<&str> {
        let i = self.fields.partition_point(|(k, _)| k.as_str() < key);
        self.fields.get(i).filter(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// All values of `key`.
    pub fn get_all<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        let i = self.fields.partition_point(|(k, _)| k.as_str() < key);
        self.fields[i..].iter().take_while(move |(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    pub fn has(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    pub fn message(&self) -> &str {
        self.get("MESSAGE").unwrap_or("")
    }

    /// Syslog priority 0 (emerg) … 7 (debug); 6 (info) when absent.
    pub fn priority(&self) -> u8 {
        self.get("PRIORITY").and_then(|p| p.trim().parse::<u8>().ok()).filter(|p| *p <= 7).unwrap_or(6)
    }

    /// Wall-clock time in microseconds since the epoch.
    pub fn timestamp_usec(&self) -> Option<i64> {
        ["_SOURCE_REALTIME_TIMESTAMP", "__REALTIME_TIMESTAMP"]
            .iter()
            .find_map(|k| self.get(k).and_then(|v| v.parse::<i64>().ok()))
    }

    pub fn transport(&self) -> &str {
        self.get("_TRANSPORT").unwrap_or("")
    }

    /// Name of the originating process.
    pub fn process(&self) -> &str {
        if let Some(p) = self.get("SYSLOG_IDENTIFIER").or_else(|| self.get("_COMM")).filter(|s| !s.is_empty()) {
            return p;
        }
        if self.transport() == "kernel" {
            "kernel"
        } else {
            ""
        }
    }

    pub fn unit(&self) -> &str {
        self.get("_SYSTEMD_USER_UNIT")
            .or_else(|| self.get("_SYSTEMD_UNIT"))
            .or_else(|| self.get("UNIT"))
            .unwrap_or("")
    }

    pub fn pid(&self) -> Option<&str> {
        self.get("_PID").or_else(|| self.get("SYSLOG_PID"))
    }

    /// Fields in display order: MESSAGE, PRIORITY first, the rest by name.
    pub fn display_fields(&self) -> Vec<(&str, &str)> {
        const FIRST: [&str; 2] = ["MESSAGE", "PRIORITY"];
        let mut out: Vec<(&str, &str)> = Vec::with_capacity(self.fields.len());
        for f in FIRST {
            out.extend(self.get_all(f).map(|v| (f, v)));
        }
        out.extend(self.fields.iter().filter(|(k, _)| !FIRST.contains(&k.as_str())).map(|(k, v)| (k.as_str(), v.as_str())));
        out
    }

    /// `FIELD=value` lines, as copied to the clipboard.
    pub fn to_export_fields(&self) -> String {
        let mut s = String::new();
        for (k, v) in &self.fields {
            s.push_str(k);
            s.push('=');
            s.push_str(v);
            s.push('\n');
        }
        s
    }
}

/// Human name of a syslog priority.
pub fn priority_name(p: u8) -> &'static str {
    match p {
        0 => "Emergency",
        1 => "Alert",
        2 => "Critical",
        3 => "Error",
        4 => "Warning",
        5 => "Notice",
        6 => "Info",
        _ => "Debug",
    }
}

/// One line of `journalctl --list-boots`.
#[derive(Debug, Clone, PartialEq)]
pub struct Boot {
    pub index: i64,
    pub id: String,
    /// µs since the epoch, when known (JSON output).
    pub first: Option<i64>,
    pub last: Option<i64>,
    /// Pre-formatted range, when parsed from the text table.
    pub range_text: Option<String>,
}

/// Parses `journalctl --list-boots -o json`. Result is newest first.
pub fn parse_boots_json(s: &str) -> Option<Vec<Boot>> {
    let v: Value = serde_json::from_str(s.trim()).ok()?;
    let arr = v.as_array()?;
    let num = |v: Option<&Value>| -> Option<i64> {
        let v = v?;
        v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    };
    let mut boots: Vec<Boot> = arr
        .iter()
        .filter_map(|b| {
            Some(Boot {
                index: num(b.get("index"))?,
                id: b.get("boot_id")?.as_str()?.to_string(),
                first: num(b.get("first_entry")),
                last: num(b.get("last_entry")),
                range_text: None,
            })
        })
        .collect();
    boots.sort_by_key(|b| std::cmp::Reverse(b.index));
    Some(boots)
}

/// Parses the plain-text `journalctl --list-boots` table. Result is newest
/// first.
pub fn parse_boots_text(s: &str) -> Vec<Boot> {
    let mut boots: Vec<Boot> = s
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let index: i64 = parts.next()?.parse().ok()?;
            let id = parts.next()?;
            if id.len() != 32 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            let rest: Vec<&str> = parts.collect();
            let range_text = if rest.is_empty() { None } else { Some(rest.join(" ").replace('—', " – ")) };
            Some(Boot { index, id: id.to_string(), first: None, last: None, range_text })
        })
        .collect();
    boots.sort_by_key(|b| std::cmp::Reverse(b.index));
    boots
}

/// Parameters of a journal query that journalctl itself can evaluate.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QueryArgs {
    /// Boot id, or `None` for the current boot.
    pub boot: Option<String>,
    pub since: Option<String>,
    pub limit: Option<usize>,
    pub reverse: bool,
    /// Show priorities up to and including this one.
    pub max_priority: Option<u8>,
    /// Field matches; consecutive entries are OR-ed together (`+`).
    pub any_of: Vec<(String, String)>,
}

impl QueryArgs {
    /// Arguments for `journalctl` (excluding the program name).
    pub fn to_args(&self) -> Vec<String> {
        let mut a = vec!["--no-pager".to_string(), "-o".into(), "json".into()];
        a.push(format!("--boot={}", self.boot.as_deref().unwrap_or("0")));
        if self.reverse {
            a.push("--reverse".into());
        }
        if let Some(s) = &self.since {
            a.push(format!("--since={s}"));
        }
        if let Some(n) = self.limit {
            a.push(format!("--lines={n}"));
        }
        if let Some(p) = self.max_priority {
            a.push(format!("--priority={p}"));
        }
        for (i, (k, v)) in self.any_of.iter().enumerate() {
            if i > 0 {
                a.push("+".into());
            }
            a.push(format!("{k}={v}"));
        }
        a
    }
}

/// The hint journalctl prints to stderr when the caller cannot read the
/// system journal.
pub fn is_limited_access_hint(stderr_line: &str) -> bool {
    let l = stderr_line.to_ascii_lowercase();
    l.contains("not seeing messages from other users and the system")
        || l.contains("no journal files were opened due to insufficient permissions")
}

/// Whether a user can read the system journal: root, or member of one of
/// the journal-reading groups. `gids` are the process's group ids and
/// `group_file` the contents of `/etc/group`.
pub fn can_read_system_journal(uid: u32, gids: &[u32], group_file: &str) -> bool {
    if uid == 0 {
        return true;
    }
    const GROUPS: [&str; 3] = ["systemd-journal", "adm", "wheel"];
    group_file.lines().any(|line| {
        let mut p = line.split(':');
        let (Some(name), _, Some(gid)) = (p.next(), p.next(), p.next()) else { return false };
        GROUPS.contains(&name) && gid.parse::<u32>().is_ok_and(|g| gids.contains(&g))
    })
}

/// Parses the `Groups:` line of `/proc/self/status`.
pub fn parse_proc_groups(status: &str) -> Vec<u32> {
    status
        .lines()
        .find_map(|l| l.strip_prefix("Groups:"))
        .map(|g| g.split_whitespace().filter_map(|n| n.parse().ok()).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/journal.jsonl");

    fn fixture() -> Vec<Entry> {
        FIXTURE.lines().filter(|l| !l.trim().is_empty()).map(|l| Entry::parse(l).unwrap()).collect()
    }

    #[test]
    fn parses_fixture() {
        let e = fixture();
        assert_eq!(e.len(), 7);
        assert_eq!(e[0].message(), "Failed to spawn the wsdd daemon");
        assert_eq!(e[0].priority(), 4);
        assert_eq!(e[0].process(), "gvfsd-wsdd");
        assert_eq!(e[0].unit(), "gvfs-daemon.service");
        assert_eq!(e[0].pid(), Some("2741705"));
        assert_eq!(e[0].timestamp_usec(), Some(1794523239432501));
    }

    #[test]
    fn byte_array_values_are_decoded() {
        let e = fixture();
        // "Hello\x1b[0m" as bytes.
        assert_eq!(e[1].message(), "Hello\u{1b}[0m");
        // Invalid UTF-8 decodes lossily.
        let e = Entry::parse(r#"{"MESSAGE":[104,105,255]}"#).unwrap();
        assert_eq!(e.message(), "hi\u{fffd}");
        let e = Entry::parse(r#"{"MESSAGE":[]}"#).unwrap();
        assert_eq!(e.message(), "");
    }

    #[test]
    fn multi_valued_and_null_fields() {
        let e = Entry::parse(r#"{"TAG":["a",[98],"c"],"BIG":null,"MESSAGE":"x"}"#).unwrap();
        assert_eq!(e.get_all("TAG").collect::<Vec<_>>(), ["a", "b", "c"]);
        assert_eq!(e.get("BIG"), Some(OMITTED));
        assert_eq!(e.get("NOPE"), None);
    }

    #[test]
    fn kernel_process_and_defaults() {
        let e = fixture();
        let k = &e[2];
        assert_eq!(k.transport(), "kernel");
        assert_eq!(k.process(), "kernel");
        assert_eq!(k.timestamp_usec(), Some(1794523000000000));
        let empty = Entry::parse("{}").unwrap();
        assert_eq!(empty.priority(), 6);
        assert_eq!(empty.process(), "");
        assert_eq!(empty.timestamp_usec(), None);
    }

    #[test]
    fn rejects_bad_lines() {
        assert!(matches!(Entry::parse("not json"), Err(ParseError::Json(_))));
        assert_eq!(Entry::parse("[1,2]"), Err(ParseError::NotAnObject));
    }

    #[test]
    fn display_order() {
        let e = Entry::from_fields([("A", "1"), ("PRIORITY", "3"), ("MESSAGE", "m"), ("Z", "2")]);
        let keys: Vec<_> = e.display_fields().into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["MESSAGE", "PRIORITY", "A", "Z"]);
        assert_eq!(e.to_export_fields(), "A=1\nMESSAGE=m\nPRIORITY=3\nZ=2\n");
    }

    #[test]
    fn boots_json() {
        let s = r#"[{"index":-1,"boot_id":"5f6a848f9e394801aebc9dce27c27a80","first_entry":1777300468694642,"last_entry":1777810668514444},
                    {"index":0,"boot_id":"05dab0ec2c864a61a03ddc8f09c59f6c","first_entry":1777810785042632,"last_entry":1778109484331422}]"#;
        let b = parse_boots_json(s).unwrap();
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].index, 0);
        assert_eq!(b[0].id, "05dab0ec2c864a61a03ddc8f09c59f6c");
        assert_eq!(b[1].first, Some(1777300468694642));
        assert!(parse_boots_json("garbage").is_none());
    }

    #[test]
    fn boots_text() {
        let s = "IDX BOOT ID                          FIRST ENTRY                 LAST ENTRY\n \
                 -1 a3075cfaef9d4224b9784b19a47025ab Tue 2026-10-13 01:23:12 EDT Sat 2026-10-31 04:42:21 EDT\n  \
                 0 920f917b2c9e49b4ae70ef5fabbe99e6 Sat 2026-10-31 04:44:04 EDT Thu 2026-11-12 17:42:41 EST\n";
        let b = parse_boots_text(s);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].index, 0);
        assert_eq!(b[0].id, "920f917b2c9e49b4ae70ef5fabbe99e6");
        assert_eq!(b[1].index, -1);
        assert!(b[1].range_text.as_deref().unwrap().starts_with("Tue 2026-10-13"));
    }

    #[test]
    fn query_args() {
        let q = QueryArgs {
            boot: Some("abc".into()),
            since: Some("2026-01-01".into()),
            limit: Some(10),
            reverse: true,
            max_priority: Some(3),
            any_of: vec![("_TRANSPORT".into(), "audit".into()), ("SYSLOG_FACILITY".into(), "4".into())],
        };
        assert_eq!(
            q.to_args(),
            [
                "--no-pager", "-o", "json", "--boot=abc", "--reverse", "--since=2026-01-01", "--lines=10",
                "--priority=3", "_TRANSPORT=audit", "+", "SYSLOG_FACILITY=4"
            ]
        );
        assert_eq!(QueryArgs::default().to_args(), ["--no-pager", "-o", "json", "--boot=0"]);
    }

    #[test]
    fn permissions() {
        let groups = "root:x:0:\nadm:x:4:syslog,danny\nsudo:x:27:danny\nsystemd-journal:x:101:\n";
        assert!(can_read_system_journal(0, &[], groups));
        assert!(can_read_system_journal(1000, &[1000, 4], groups));
        assert!(can_read_system_journal(1000, &[101], groups));
        assert!(!can_read_system_journal(1000, &[1000, 27], groups));
        assert_eq!(parse_proc_groups("Name:\tx\nGroups:\t4 24 27 \nX: 1"), [4, 24, 27]);
        assert!(is_limited_access_hint(
            "Hint: You are currently not seeing messages from other users and the system."
        ));
        assert!(!is_limited_access_hint("-- No entries --"));
    }
}
