//! First-run state, persisted as `state.json` in the app's config dir.

use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "state.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    /// The user reached the last page and closed the tour.
    Completed,
    /// The user declined or closed the tour early.
    Skipped,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub completed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl State {
    /// Loads state from `path`. A missing, empty or corrupt file yields the
    /// default ("not completed") state.
    pub fn load(path: &Path) -> State {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Writes state atomically (temp file + rename).
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        std::fs::write(&tmp, json + "\n")?;
        std::fs::rename(&tmp, path)
    }

    /// Records that the tour ended with `outcome`. A completion is never
    /// downgraded to a skip.
    pub fn finish(&mut self, outcome: Outcome, version: &str) {
        self.completed = true;
        if self.outcome != Some(Outcome::Completed) {
            self.outcome = Some(outcome);
        }
        self.version = Some(version.to_string());
    }
}

/// Whether the window should be shown, given `--if-first-run`.
pub fn should_show(if_first_run: bool, state: &State) -> bool {
    !if_first_run || !state.completed
}

/// `$XDG_CONFIG_HOME/helix-tour/state.json`.
pub fn default_path() -> PathBuf {
    helix_app_common::config_dir("helix-tour").join(FILE_NAME)
}

/// Loads, updates and saves the state at `path`.
pub fn record(path: &Path, outcome: Outcome, version: &str) -> io::Result<State> {
    let mut state = State::load(path);
    state.finish(outcome, version);
    state.save(path)?;
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("helix-tour-state-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn missing_or_corrupt_means_first_run() {
        let dir = temp_dir("corrupt");
        let path = dir.join(FILE_NAME);
        assert_eq!(State::load(&path), State::default());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "").unwrap();
        assert!(!State::load(&path).completed);
        std::fs::write(&path, "{not json").unwrap();
        assert!(!State::load(&path).completed);
        std::fs::write(&path, "{}").unwrap();
        assert!(!State::load(&path).completed);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn should_show_logic() {
        let fresh = State::default();
        let mut done = State::default();
        done.finish(Outcome::Skipped, "0.1.0");
        assert!(should_show(false, &fresh));
        assert!(should_show(false, &done));
        assert!(should_show(true, &fresh));
        assert!(!should_show(true, &done));
    }

    #[test]
    fn completion_is_not_downgraded() {
        let mut s = State::default();
        s.finish(Outcome::Completed, "0.1.0");
        s.finish(Outcome::Skipped, "0.2.0");
        assert_eq!(s.outcome, Some(Outcome::Completed));
        assert_eq!(s.version.as_deref(), Some("0.2.0"));

        let mut s = State::default();
        s.finish(Outcome::Skipped, "0.1.0");
        s.finish(Outcome::Completed, "0.1.0");
        assert_eq!(s.outcome, Some(Outcome::Completed));
    }

    #[test]
    fn record_round_trip() {
        let dir = temp_dir("record");
        let path = dir.join("nested").join(FILE_NAME);
        let saved = record(&path, Outcome::Skipped, "0.1.0").unwrap();
        assert!(saved.completed);
        let loaded = State::load(&path);
        assert_eq!(loaded, saved);
        assert!(!should_show(true, &loaded));
        let json = std::fs::read_to_string(&path).unwrap();
        assert!(json.contains("\"outcome\": \"skipped\""), "{json}");
        assert!(!path.with_extension("json.tmp").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
