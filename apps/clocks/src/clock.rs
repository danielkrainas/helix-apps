//! Injectable time sources.
//!
//! Logic modules never read the system clock themselves; they take the
//! wall-clock time (`DateTime<Utc>`) and a monotonic timestamp (`Duration`
//! since an arbitrary epoch) as arguments. The UI obtains both from a
//! [`Clock`], so tests can substitute a fake one.

use chrono::{DateTime, Utc};
use std::time::{Duration, Instant};

pub trait Clock {
    /// Current wall-clock time.
    fn now(&self) -> DateTime<Utc>;
    /// Monotonic time since an arbitrary, fixed epoch.
    fn mono(&self) -> Duration;
}

/// The real system clock.
pub struct SystemClock {
    epoch: Instant,
}

impl Default for SystemClock {
    fn default() -> Self {
        SystemClock { epoch: Instant::now() }
    }
}

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
    fn mono(&self) -> Duration {
        self.epoch.elapsed()
    }
}

/// A manually advanced clock for tests.
#[cfg(test)]
pub struct FakeClock {
    pub now: std::cell::Cell<DateTime<Utc>>,
    pub mono: std::cell::Cell<Duration>,
}

#[cfg(test)]
impl FakeClock {
    pub fn new(now: DateTime<Utc>) -> Self {
        FakeClock { now: std::cell::Cell::new(now), mono: std::cell::Cell::new(Duration::ZERO) }
    }
    pub fn advance(&self, d: Duration) {
        self.now.set(self.now.get() + chrono::Duration::from_std(d).unwrap());
        self.mono.set(self.mono.get() + d);
    }
}

#[cfg(test)]
impl Clock for FakeClock {
    fn now(&self) -> DateTime<Utc> {
        self.now.get()
    }
    fn mono(&self) -> Duration {
        self.mono.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn fake_clock_advances_both() {
        let c = FakeClock::new(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap());
        c.advance(Duration::from_secs(90));
        assert_eq!(c.now(), Utc.with_ymd_and_hms(2024, 1, 1, 0, 1, 30).unwrap());
        assert_eq!(c.mono(), Duration::from_secs(90));
    }

    #[test]
    fn system_clock_is_monotonic() {
        let c = SystemClock::default();
        let a = c.mono();
        let b = c.mono();
        assert!(b >= a);
    }
}
