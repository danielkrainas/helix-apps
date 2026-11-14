//! Photo countdown timer state machine (driven by 1-second ticks).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Countdown {
    Idle,
    Running { remaining: u32 },
}

/// What the UI should do after a state change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Nothing to do.
    None,
    /// Take the photo now.
    Capture,
    /// Show `n` on the overlay and keep ticking every second.
    Show(u32),
    /// The countdown was cancelled; hide the overlay.
    Cancelled,
}

/// Allowed timer settings in seconds (0 = off).
pub const DELAYS: [u32; 4] = [0, 3, 5, 10];

pub fn normalize_delay(secs: u32) -> u32 {
    if DELAYS.contains(&secs) {
        secs
    } else {
        0
    }
}

impl Countdown {
    pub fn is_running(self) -> bool {
        matches!(self, Countdown::Running { .. })
    }

    /// Shutter pressed with the given delay.
    pub fn press(&mut self, delay: u32) -> Action {
        match *self {
            Countdown::Running { .. } => {
                *self = Countdown::Idle;
                Action::Cancelled
            }
            Countdown::Idle if delay == 0 => Action::Capture,
            Countdown::Idle => {
                *self = Countdown::Running { remaining: delay };
                Action::Show(delay)
            }
        }
    }

    /// One second elapsed.
    pub fn tick(&mut self) -> Action {
        match *self {
            Countdown::Idle => Action::None,
            Countdown::Running { remaining } if remaining <= 1 => {
                *self = Countdown::Idle;
                Action::Capture
            }
            Countdown::Running { remaining } => {
                *self = Countdown::Running {
                    remaining: remaining - 1,
                };
                Action::Show(remaining - 1)
            }
        }
    }

    pub fn cancel(&mut self) -> Action {
        if self.is_running() {
            *self = Countdown::Idle;
            Action::Cancelled
        } else {
            Action::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_delay_captures_immediately() {
        let mut c = Countdown::Idle;
        assert_eq!(c.press(0), Action::Capture);
        assert_eq!(c, Countdown::Idle);
    }

    #[test]
    fn three_second_countdown() {
        let mut c = Countdown::Idle;
        assert_eq!(c.press(3), Action::Show(3));
        assert_eq!(c.tick(), Action::Show(2));
        assert_eq!(c.tick(), Action::Show(1));
        assert_eq!(c.tick(), Action::Capture);
        assert_eq!(c, Countdown::Idle);
        assert_eq!(c.tick(), Action::None);
    }

    #[test]
    fn pressing_again_cancels() {
        let mut c = Countdown::Idle;
        c.press(5);
        c.tick();
        assert_eq!(c.press(5), Action::Cancelled);
        assert!(!c.is_running());
        assert_eq!(c.tick(), Action::None);
    }

    #[test]
    fn cancel_when_idle_is_noop() {
        let mut c = Countdown::Idle;
        assert_eq!(c.cancel(), Action::None);
        c.press(10);
        assert_eq!(c.cancel(), Action::Cancelled);
    }

    #[test]
    fn delays_are_normalized() {
        assert_eq!(normalize_delay(5), 5);
        assert_eq!(normalize_delay(7), 0);
    }
}
