//! Countdown timers on a monotonic timestamp.

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Stopped,
    Running { end: Duration },
    Paused { remaining: Duration },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Timer {
    pub name: String,
    pub duration: Duration,
    pub state: State,
}

/// Preset durations in seconds.
pub const PRESETS: [u64; 8] = [60, 120, 180, 300, 600, 900, 1800, 3600];

impl Timer {
    pub fn new(name: impl Into<String>, duration: Duration) -> Self {
        Timer {
            name: name.into(),
            duration,
            state: State::Stopped,
        }
    }

    pub fn remaining(&self, mono: Duration) -> Duration {
        match self.state {
            State::Stopped => self.duration,
            State::Running { end } => end.saturating_sub(mono),
            State::Paused { remaining } => remaining,
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self.state, State::Running { .. })
    }

    /// Fraction of the duration that has elapsed, 0.0–1.0.
    pub fn progress(&self, mono: Duration) -> f64 {
        if self.duration.is_zero() {
            return 1.0;
        }
        1.0 - self.remaining(mono).as_secs_f64() / self.duration.as_secs_f64()
    }

    pub fn start(&mut self, mono: Duration) {
        let remaining = match self.state {
            State::Running { .. } => return,
            State::Stopped => self.duration,
            State::Paused { remaining } => remaining,
        };
        self.state = State::Running {
            end: mono + remaining,
        };
    }

    pub fn pause(&mut self, mono: Duration) {
        if self.is_running() {
            self.state = State::Paused {
                remaining: self.remaining(mono),
            };
        }
    }

    pub fn reset(&mut self) {
        self.state = State::Stopped;
    }

    /// Returns true exactly once when a running timer reaches zero; the
    /// timer is then reset to its full duration.
    pub fn tick(&mut self, mono: Duration) -> bool {
        if let State::Running { end } = self.state {
            if mono >= end {
                self.state = State::Stopped;
                return true;
            }
        }
        false
    }
}

/// Countdown text, rounding up to whole seconds: "MM:SS" or "H:MM:SS".
pub fn format_countdown(d: Duration) -> String {
    let total = d.as_millis().div_ceil(1000) as u64;
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// Human label for a preset: "5 min", "1 h", "1 h 30 min", "45 s".
pub fn describe_duration(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
    let mut parts = Vec::new();
    if h > 0 {
        parts.push(format!("{h} h"));
    }
    if m > 0 {
        parts.push(format!("{m} min"));
    }
    if s > 0 || parts.is_empty() {
        parts.push(format!("{s} s"));
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::{Clock, FakeClock};
    use chrono::{TimeZone, Utc};

    #[test]
    fn countdown_pause_and_finish() {
        let c = FakeClock::new(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap());
        let mut t = Timer::new("Tea", Duration::from_secs(180));
        assert_eq!(t.remaining(c.mono()), Duration::from_secs(180));
        t.start(c.mono());
        c.advance(Duration::from_secs(60));
        assert_eq!(t.remaining(c.mono()), Duration::from_secs(120));
        assert!((t.progress(c.mono()) - 1.0 / 3.0).abs() < 1e-9);
        t.pause(c.mono());
        c.advance(Duration::from_secs(1000));
        assert!(!t.tick(c.mono()));
        assert_eq!(t.remaining(c.mono()), Duration::from_secs(120));
        t.start(c.mono());
        c.advance(Duration::from_millis(119_999));
        assert!(!t.tick(c.mono()));
        assert_eq!(format_countdown(t.remaining(c.mono())), "00:01");
        c.advance(Duration::from_millis(1));
        assert!(t.tick(c.mono()), "fires at zero");
        assert_eq!(t.state, State::Stopped);
        assert_eq!(
            t.remaining(c.mono()),
            Duration::from_secs(180),
            "resets to full duration"
        );
        assert!(!t.tick(c.mono()), "fires only once");
    }

    #[test]
    fn late_tick_still_fires() {
        let c = FakeClock::new(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap());
        let mut t = Timer::new("", Duration::from_secs(5));
        t.start(c.mono());
        c.advance(Duration::from_secs(60));
        assert_eq!(t.remaining(c.mono()), Duration::ZERO);
        assert!(t.tick(c.mono()));
    }

    #[test]
    fn reset_and_zero() {
        let mut t = Timer::new("", Duration::from_secs(5));
        t.start(Duration::ZERO);
        t.reset();
        assert_eq!(t.state, State::Stopped);
        let z = Timer::new("", Duration::ZERO);
        assert_eq!(z.progress(Duration::ZERO), 1.0);
    }

    #[test]
    fn formatting() {
        assert_eq!(format_countdown(Duration::from_secs(0)), "00:00");
        assert_eq!(format_countdown(Duration::from_millis(59_001)), "01:00");
        assert_eq!(format_countdown(Duration::from_secs(3725)), "1:02:05");
        assert_eq!(describe_duration(300), "5 min");
        assert_eq!(describe_duration(3600), "1 h");
        assert_eq!(describe_duration(5400), "1 h 30 min");
        assert_eq!(describe_duration(45), "45 s");
        assert_eq!(describe_duration(0), "0 s");
    }
}
