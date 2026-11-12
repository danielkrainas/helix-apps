//! Saved browsing session (normal windows only).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabState {
    pub uri: String,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowState {
    pub tabs: Vec<TabState>,
    #[serde(default)]
    pub selected: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    #[serde(default)]
    pub windows: Vec<WindowState>,
    /// False while the browser is running; set on clean exit.
    #[serde(default = "yes")]
    pub clean_exit: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestorePolicy {
    Always,
    Crashed,
    Never,
}

impl RestorePolicy {
    pub const NICKS: [&'static str; 3] = ["always", "crashed", "never"];

    pub fn from_nick(s: &str) -> Self {
        match s {
            "crashed" => RestorePolicy::Crashed,
            "never" => RestorePolicy::Never,
            _ => RestorePolicy::Always,
        }
    }

    pub fn nick(self) -> &'static str {
        match self {
            RestorePolicy::Always => "always",
            RestorePolicy::Crashed => "crashed",
            RestorePolicy::Never => "never",
        }
    }
}

/// URIs that are not worth saving.
pub fn savable(uri: &str) -> bool {
    !uri.is_empty() && uri != "about:blank"
}

impl Session {
    /// Windows to restore under `policy`, dropping empty windows and
    /// clamping selections.
    pub fn to_restore(&self, policy: RestorePolicy) -> Vec<WindowState> {
        let restore = match policy {
            RestorePolicy::Always => true,
            RestorePolicy::Crashed => !self.clean_exit,
            RestorePolicy::Never => false,
        };
        if !restore {
            return Vec::new();
        }
        self.windows
            .iter()
            .filter_map(|w| {
                let tabs: Vec<TabState> =
                    w.tabs.iter().filter(|t| savable(&t.uri)).cloned().collect();
                if tabs.is_empty() {
                    return None;
                }
                let selected = w.selected.min(tabs.len() - 1);
                Some(WindowState { tabs, selected })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(u: &str) -> TabState {
        TabState {
            uri: u.into(),
            title: String::new(),
        }
    }

    #[test]
    fn restore_policies() {
        let s = Session {
            windows: vec![
                WindowState {
                    tabs: vec![tab("https://a/"), tab("about:blank"), tab("https://b/")],
                    selected: 9,
                },
                WindowState {
                    tabs: vec![tab("about:blank")],
                    selected: 0,
                },
            ],
            clean_exit: true,
        };
        let w = s.to_restore(RestorePolicy::Always);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].tabs.len(), 2);
        assert_eq!(w[0].selected, 1);
        assert!(s.to_restore(RestorePolicy::Never).is_empty());
        assert!(s.to_restore(RestorePolicy::Crashed).is_empty());
        let crashed = Session {
            clean_exit: false,
            ..s.clone()
        };
        assert_eq!(crashed.to_restore(RestorePolicy::Crashed).len(), 1);
    }

    #[test]
    fn json_roundtrip() {
        let dir = crate::store::temp_dir("session");
        let p = dir.join("session.json");
        let s = Session {
            windows: vec![WindowState {
                tabs: vec![tab("https://x/")],
                selected: 0,
            }],
            clean_exit: false,
        };
        crate::store::write_atomic(&p, &crate::store::to_json(&s)).unwrap();
        let back: Session = crate::store::load(&p);
        assert_eq!(back, s);
        let empty: Session = serde_json::from_str("{}").unwrap();
        assert!(empty.clean_exit);
        assert_eq!(RestorePolicy::from_nick("bogus"), RestorePolicy::Always);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
