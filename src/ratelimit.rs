//! Sliding-window rate limiter for unauthenticated or cheap-to-call surfaces
//! (inbound webhooks). In-process: this binary is a single instance per
//! database, and the limit protects the process, not a distributed quota.
//!
//! Keys are attacker-controlled (event names, client addresses), so the map
//! is bounded: expired keys are evicted, and past `MAX_KEYS` live keys new
//! ones are refused rather than allocated.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Upper bound on distinct keys tracked at once.
const MAX_KEYS: usize = 10_000;
const WINDOW: Duration = Duration::from_secs(60);

pub struct SlidingWindow {
    per_min: u32,
    hits: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl SlidingWindow {
    /// `per_min == 0` disables the limiter (every call is allowed).
    pub fn new(per_min: u32) -> Self {
        Self {
            per_min,
            hits: Mutex::new(HashMap::new()),
        }
    }

    /// Record one attempt for `key` at `now` and say whether it is within the
    /// limit. Attempts are counted whether or not they are allowed, so a flood
    /// stays refused until it stops.
    pub fn allow_at(&self, key: &str, now: Instant) -> bool {
        if self.per_min == 0 {
            return true;
        }
        let mut hits = match self.hits.lock() {
            Ok(h) => h,
            Err(poisoned) => poisoned.into_inner(),
        };
        if !hits.contains_key(key) {
            if hits.len() >= MAX_KEYS {
                // Drop every key whose newest hit has left the window.
                hits.retain(|_, q| {
                    q.back()
                        .map(|last| now.duration_since(*last) < WINDOW)
                        .unwrap_or(false)
                });
            }
            if hits.len() >= MAX_KEYS {
                // Still full of live keys: refuse rather than allocate. Live
                // keys keep working; a flood of fresh names cannot grow memory.
                return false;
            }
        }
        let q = hits.entry(key.to_string()).or_default();
        while let Some(front) = q.front() {
            if now.duration_since(*front) >= WINDOW {
                q.pop_front();
            } else {
                break;
            }
        }
        let allowed = (q.len() as u32) < self.per_min;
        q.push_back(now);
        // A queue never needs more than one window of refused attempts.
        let cap = (self.per_min as usize).saturating_mul(2).max(1);
        if q.len() > cap {
            let excess = q.len() - cap;
            q.drain(..excess);
        }
        allowed
    }

    pub fn allow(&self, key: &str) -> bool {
        self.allow_at(key, Instant::now())
    }

    /// Number of keys currently tracked.
    #[cfg(test)]
    pub fn tracked_keys(&self) -> usize {
        self.hits.lock().map(|h| h.len()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_up_to_the_limit_then_refuses_until_the_window_slides() {
        let l = SlidingWindow::new(3);
        let t0 = Instant::now();
        assert!(l.allow_at("ev", t0));
        assert!(l.allow_at("ev", t0 + Duration::from_secs(1)));
        assert!(l.allow_at("ev", t0 + Duration::from_secs(2)));
        assert!(
            !l.allow_at("ev", t0 + Duration::from_secs(3)),
            "4th within a minute"
        );
        assert!(
            l.allow_at("other", t0 + Duration::from_secs(3)),
            "keys are independent"
        );
        // Refused attempts count too: still refused 30s later.
        assert!(!l.allow_at("ev", t0 + Duration::from_secs(30)));
        // Well after the window everything before has expired.
        assert!(l.allow_at("ev", t0 + Duration::from_secs(200)));
    }

    #[test]
    fn zero_disables() {
        let l = SlidingWindow::new(0);
        for _ in 0..1000 {
            assert!(l.allow("ev"));
        }
    }

    #[test]
    fn the_key_set_is_bounded_and_expired_keys_are_evicted() {
        let l = SlidingWindow::new(5);
        let t0 = Instant::now();
        for i in 0..MAX_KEYS {
            assert!(l.allow_at(&format!("k{i}"), t0));
        }
        assert_eq!(l.tracked_keys(), MAX_KEYS);
        // Full of live keys: a fresh key is refused, a known one still works.
        assert!(!l.allow_at("fresh", t0 + Duration::from_secs(1)));
        assert!(l.allow_at("k1", t0 + Duration::from_secs(1)));
        // Once the window has passed, the stale keys are evicted for a newcomer.
        assert!(l.allow_at("fresh", t0 + Duration::from_secs(120)));
        assert!(l.tracked_keys() < MAX_KEYS);
    }
}
