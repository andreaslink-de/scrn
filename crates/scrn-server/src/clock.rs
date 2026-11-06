//! Injectable clock, so tests can control time (invite and session expiry,
//! rate limit windows, later link expiry).

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Source of the current time in Unix seconds.
pub trait Clock: Send + Sync {
    fn now(&self) -> i64;
}

/// The real system clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
    }
}

/// A clock that only moves when told to. Meant for tests.
#[derive(Debug, Default)]
pub struct ManualClock(AtomicI64);

impl ManualClock {
    pub fn new(now: i64) -> Self {
        Self(AtomicI64::new(now))
    }

    pub fn set(&self, now: i64) {
        self.0.store(now, Ordering::SeqCst);
    }

    pub fn advance(&self, seconds: i64) {
        self.0.fetch_add(seconds, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_clock_is_after_2025() {
        assert!(SystemClock.now() > 1_735_689_600);
    }

    #[test]
    fn manual_clock_moves_on_request() {
        let clock = ManualClock::new(100);
        assert_eq!(clock.now(), 100);
        clock.advance(5);
        assert_eq!(clock.now(), 105);
        clock.set(1);
        assert_eq!(clock.now(), 1);
    }
}
