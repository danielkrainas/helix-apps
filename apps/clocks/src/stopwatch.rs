//! Stopwatch logic on a monotonic timestamp.

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Reset,
    Running {
        started_at: Duration,
        base: Duration,
    },
    Paused {
        elapsed: Duration,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lap {
    /// 1-based.
    pub number: usize,
    pub split: Duration,
    pub total: Duration,
    /// Split minus the previous lap's split, in milliseconds.
    pub delta_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct Stopwatch {
    pub state: State,
    /// Cumulative elapsed time at each lap.
    totals: Vec<Duration>,
}

impl Default for Stopwatch {
    fn default() -> Self {
        Stopwatch {
            state: State::Reset,
            totals: Vec::new(),
        }
    }
}

impl Stopwatch {
    pub fn elapsed(&self, mono: Duration) -> Duration {
        match self.state {
            State::Reset => Duration::ZERO,
            State::Running { started_at, base } => base + mono.saturating_sub(started_at),
            State::Paused { elapsed } => elapsed,
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self.state, State::Running { .. })
    }

    /// Starts from reset, or resumes from pause.
    pub fn start(&mut self, mono: Duration) {
        let base = match self.state {
            State::Running { .. } => return,
            State::Reset => Duration::ZERO,
            State::Paused { elapsed } => elapsed,
        };
        self.state = State::Running {
            started_at: mono,
            base,
        };
    }

    pub fn pause(&mut self, mono: Duration) {
        if self.is_running() {
            self.state = State::Paused {
                elapsed: self.elapsed(mono),
            };
        }
    }

    /// Start ↔ pause.
    pub fn toggle(&mut self, mono: Duration) {
        if self.is_running() {
            self.pause(mono)
        } else {
            self.start(mono)
        }
    }

    pub fn reset(&mut self) {
        self.state = State::Reset;
        self.totals.clear();
    }

    /// Records a lap (only while running) and returns it.
    pub fn lap(&mut self, mono: Duration) -> Option<Lap> {
        if !self.is_running() {
            return None;
        }
        self.totals.push(self.elapsed(mono));
        self.laps().pop()
    }

    /// The current, still-open lap's split.
    pub fn current_split(&self, mono: Duration) -> Duration {
        self.elapsed(mono)
            .saturating_sub(self.totals.last().copied().unwrap_or_default())
    }

    /// All laps, oldest first.
    pub fn laps(&self) -> Vec<Lap> {
        let mut prev_total = Duration::ZERO;
        let mut prev_split: Option<Duration> = None;
        let mut out = Vec::with_capacity(self.totals.len());
        for (i, &total) in self.totals.iter().enumerate() {
            let split = total - prev_total;
            out.push(Lap {
                number: i + 1,
                split,
                total,
                delta_ms: prev_split.map(|p| split.as_millis() as i64 - p.as_millis() as i64),
            });
            prev_total = total;
            prev_split = Some(split);
        }
        out
    }
}

/// "MM:SS.d" (or "H:MM:SS.d"); with `centis`, two decimal digits.
pub fn format_elapsed(d: Duration, centis: bool) -> String {
    let cs = d.as_millis() / 10;
    let (h, m, s, c) = (cs / 360_000, (cs / 6000) % 60, (cs / 100) % 60, cs % 100);
    let frac = if centis {
        format!("{c:02}")
    } else {
        format!("{}", c / 10)
    };
    if h > 0 {
        format!("{h}:{m:02}:{s:02}.{frac}")
    } else {
        format!("{m:02}:{s:02}.{frac}")
    }
}

/// "+0:01.23" / "−0:00.40".
pub fn format_delta(ms: i64) -> String {
    let sign = if ms < 0 { '−' } else { '+' };
    let cs = ms.unsigned_abs() / 10;
    let (m, s, c) = (cs / 6000, (cs / 100) % 60, cs % 100);
    format!("{sign}{m}:{s:02}.{c:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::{Clock, FakeClock};
    use chrono::{TimeZone, Utc};

    fn secs(s: f64) -> Duration {
        Duration::from_secs_f64(s)
    }

    #[test]
    fn start_pause_resume_reset() {
        let c = FakeClock::new(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap());
        let mut sw = Stopwatch::default();
        assert_eq!(sw.elapsed(c.mono()), Duration::ZERO);
        sw.start(c.mono());
        c.advance(secs(1.5));
        assert_eq!(sw.elapsed(c.mono()), secs(1.5));
        sw.pause(c.mono());
        c.advance(secs(10.0));
        assert_eq!(
            sw.elapsed(c.mono()),
            secs(1.5),
            "paused time does not count"
        );
        sw.toggle(c.mono());
        c.advance(secs(0.5));
        assert_eq!(sw.elapsed(c.mono()), secs(2.0));
        sw.start(c.mono()); // no-op while running
        assert_eq!(sw.elapsed(c.mono()), secs(2.0));
        sw.reset();
        assert_eq!(sw.state, State::Reset);
        assert!(sw.laps().is_empty());
    }

    #[test]
    fn laps_and_deltas() {
        let c = FakeClock::new(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap());
        let mut sw = Stopwatch::default();
        assert!(sw.lap(c.mono()).is_none(), "no laps while reset");
        sw.start(c.mono());
        c.advance(secs(10.0));
        let l1 = sw.lap(c.mono()).unwrap();
        assert_eq!(
            (l1.number, l1.split, l1.total, l1.delta_ms),
            (1, secs(10.0), secs(10.0), None)
        );
        c.advance(secs(8.5));
        let l2 = sw.lap(c.mono()).unwrap();
        assert_eq!(
            (l2.split, l2.total, l2.delta_ms),
            (secs(8.5), secs(18.5), Some(-1500))
        );
        c.advance(secs(9.0));
        assert_eq!(sw.current_split(c.mono()), secs(9.0));
        sw.pause(c.mono());
        assert!(sw.lap(c.mono()).is_none(), "no laps while paused");
        assert_eq!(sw.laps().len(), 2);
    }

    #[test]
    fn formatting() {
        assert_eq!(format_elapsed(secs(0.0), false), "00:00.0");
        assert_eq!(format_elapsed(secs(65.47), false), "01:05.4");
        assert_eq!(format_elapsed(secs(65.47), true), "01:05.47");
        assert_eq!(format_elapsed(secs(3723.05), true), "1:02:03.05");
        assert_eq!(format_delta(1230), "+0:01.23");
        assert_eq!(format_delta(-400), "−0:00.40");
        assert_eq!(format_delta(61_000), "+1:01.00");
    }
}
