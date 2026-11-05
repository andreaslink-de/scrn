//! In-memory fixed-window rate limiter keyed by strings (IP addresses,
//! usernames). State is lost on restart, which is acceptable for a single
//! server instance.

use std::collections::HashMap;
use std::sync::Mutex;

/// Allows at most `limit` events per key within `window` seconds.
#[derive(Debug)]
pub struct RateLimiter {
    limit: u32,
    window: i64,
    entries: Mutex<HashMap<String, Entry>>,
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    window_start: i64,
    count: u32,
}

/// Number of keys after which expired entries are swept.
const SWEEP_THRESHOLD: usize = 10_000;

impl RateLimiter {
    pub fn new(limit: u32, window_seconds: i64) -> Self {
        Self {
            limit,
            window: window_seconds,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// True if `key` has used up its budget in the current window.
    pub fn is_limited(&self, key: &str, now: i64) -> bool {
        let entries = self.lock();
        entries
            .get(key)
            .is_some_and(|e| now - e.window_start < self.window && e.count >= self.limit)
    }

    /// Records one event for `key`.
    pub fn record(&self, key: &str, now: i64) {
        let mut entries = self.lock();
        if entries.len() >= SWEEP_THRESHOLD {
            let window = self.window;
            entries.retain(|_, e| now - e.window_start < window);
        }
        let entry = entries.entry(key.to_owned()).or_insert(Entry {
            window_start: now,
            count: 0,
        });
        if now - entry.window_start >= self.window {
            *entry = Entry {
                window_start: now,
                count: 0,
            };
        }
        entry.count = entry.count.saturating_add(1);
    }

    /// Forgets all events of `key`.
    pub fn reset(&self, key: &str) {
        self.lock().remove(key);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        // A panic while holding the lock cannot leave the map inconsistent,
        // so recover from poisoning instead of propagating it.
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_within_window_and_recovers_after() {
        let limiter = RateLimiter::new(3, 60);
        for _ in 0..3 {
            assert!(!limiter.is_limited("k", 100));
            limiter.record("k", 100);
        }
        assert!(limiter.is_limited("k", 159));
        assert!(!limiter.is_limited("other", 159));
        assert!(!limiter.is_limited("k", 160));
        limiter.record("k", 160);
        assert!(!limiter.is_limited("k", 160));
    }

    #[test]
    fn reset_clears_a_key() {
        let limiter = RateLimiter::new(1, 60);
        limiter.record("k", 100);
        assert!(limiter.is_limited("k", 100));
        limiter.reset("k");
        assert!(!limiter.is_limited("k", 100));
    }
}
